using System.Runtime.InteropServices;
using Reshot.Capture.Interop;
using Reshot.Core.Diagnostics;
using Vortice.Direct3D;
using Vortice.Direct3D11;
using Vortice.DXGI;
using Windows.Foundation;
using Windows.Graphics.Capture;
using Windows.Graphics.DirectX;
using WinRT;
using static Vortice.Direct3D11.D3D11;
using WinRTDirect3DDevice = Windows.Graphics.DirectX.Direct3D11.IDirect3DDevice;

namespace Reshot.Capture;

/// <summary>
/// Windows.Graphics.Capture implementation of <see cref="IScreenCaptureService"/>.
/// Grabs one frame from each monitor via a free-threaded frame pool and blits them
/// into a single virtual-desktop BGRA buffer. All GPU resources are created and
/// torn down within one call so nothing lingers in the background (ARCHITECTURE §7).
/// </summary>
public sealed class WgcScreenCaptureService : IScreenCaptureService
{
    /// <summary>
    /// How long to wait for a capture session's first frame. WGC normally delivers it
    /// within a refresh; a monitor that never delivers one is a fullscreen game refusing
    /// the capture, and the point of the wait is to reach the CreateForWindow fallback,
    /// not to keep hoping. The old three seconds per monitor turned that case into a
    /// visible hang before the fallback ever ran.
    /// </summary>
    private static readonly TimeSpan FirstFrameTimeout = TimeSpan.FromMilliseconds(1200);

    internal static readonly FeatureLevel[] FeatureLevels =
    {
        FeatureLevel.Level_11_1,
        FeatureLevel.Level_11_0,
        FeatureLevel.Level_10_1,
        FeatureLevel.Level_10_0,
    };

    public CapturedFrame SnapshotAllMonitors()
        => SnapshotAllMonitors(CaptureNative.EnumerateMonitors());

    internal CapturedFrame SnapshotAllMonitors(
        IReadOnlyList<CaptureNative.MonitorHandle> monitors)
    {
        if (!GraphicsCaptureSession.IsSupported())
            throw new NotSupportedException("Windows.Graphics.Capture is not supported on this system.");

        var vLeft = CaptureNative.GetSystemMetrics(CaptureNative.SM_XVIRTUALSCREEN);
        var vTop = CaptureNative.GetSystemMetrics(CaptureNative.SM_YVIRTUALSCREEN);
        var vWidth = CaptureNative.GetSystemMetrics(CaptureNative.SM_CXVIRTUALSCREEN);
        var vHeight = CaptureNative.GetSystemMetrics(CaptureNative.SM_CYVIRTUALSCREEN);

        if (monitors.Count == 0)
            throw new InvalidOperationException("No monitors found to capture.");

        using var bufferLease = CapturedFrame.RentBuffer(checked(vWidth * vHeight * 4));
        var buffer = bufferLease.Buffer;
        var capturedMonitors = new List<CapturedMonitor>(monitors.Count);

        // Remember the game HWND *before* we start capturing — exclusive-fullscreen
        // titles often fail monitor WGC, but CreateForWindow still works.
        var foregroundHwnd = CaptureNative.GetForegroundWindow();

        D3D11CreateDevice(
            null,
            DriverType.Hardware,
            DeviceCreationFlags.BgraSupport,
            FeatureLevels,
            out var device).CheckError();

        if (device is null)
            throw new InvalidOperationException("D3D11CreateDevice returned a null device.");

        using (device)
        using (var context = device.ImmediateContext)
        {
            var winrtDevice = CreateWinRtDevice(device);
            try
            {
                foreach (var monitor in monitors)
                {
                    try
                    {
                        CaptureOneMonitor(device, context, winrtDevice, monitor, buffer, vLeft, vTop, vWidth);
                    }
                    catch (Exception ex) when (foregroundHwnd != IntPtr.Zero)
                    {
                        Log.Warn($"Monitor WGC failed at ({monitor.Bounds.Left},{monitor.Bounds.Top}): {ex.Message}. Trying foreground window.");
                        if (!TryCaptureForegroundWindow(
                                device, context, winrtDevice, foregroundHwnd, monitor,
                                buffer, vLeft, vTop, vWidth))
                            throw;
                    }
                    capturedMonitors.Add(new CapturedMonitor(
                        monitor.Bounds.Left, monitor.Bounds.Top,
                        monitor.Bounds.Width, monitor.Bounds.Height,
                        monitor.IsPrimary));
                }
            }
            finally
            {
                (winrtDevice as IDisposable)?.Dispose();
            }
        }

        Log.Info($"Capture: {monitors.Count} monitor(s) → {vWidth}x{vHeight} virtual desktop @ ({vLeft},{vTop}).");

        return bufferLease.CreateFrame(vWidth, vHeight, vLeft, vTop, capturedMonitors);
    }

    public WgcMonitorStream StartMonitorStream(int screenX, int screenY)
    {
        if (!GraphicsCaptureSession.IsSupported())
            throw new NotSupportedException("Windows.Graphics.Capture is not supported on this system.");

        var monitors = CaptureNative.EnumerateMonitors();
        var target = monitors.FirstOrDefault(m =>
            screenX >= m.Bounds.Left && screenX < m.Bounds.Right &&
            screenY >= m.Bounds.Top && screenY < m.Bounds.Bottom);

        // Fall back to the primary (or first) monitor if the point is off-screen.
        if (target.Handle == IntPtr.Zero)
            target = monitors.FirstOrDefault(m => m.IsPrimary);
        if (target.Handle == IntPtr.Zero && monitors.Count > 0)
            target = monitors[0];
        if (target.Handle == IntPtr.Zero)
            throw new InvalidOperationException("No monitor found for the recording region.");

        return new WgcMonitorStream(
            target.Handle, target.Bounds.Left, target.Bounds.Top, target.Bounds.Width, target.Bounds.Height);
    }

    /// <summary>Wraps the D3D11 device as a WinRT IDirect3DDevice for the capture API.</summary>
    internal static WinRTDirect3DDevice CreateWinRtDevice(ID3D11Device device)
    {
        using var dxgiDevice = device.QueryInterface<IDXGIDevice>();
        var hr = CaptureNative.CreateDirect3D11DeviceFromDXGIDevice(dxgiDevice.NativePointer, out var inspectable);
        if (hr < 0)
            throw Marshal.GetExceptionForHR(hr) ?? new COMException("CreateDirect3D11DeviceFromDXGIDevice failed", hr);

        try
        {
            return MarshalInterface<WinRTDirect3DDevice>.FromAbi(inspectable);
        }
        finally
        {
            Marshal.Release(inspectable);
        }
    }

    private static void CaptureOneMonitor(
        ID3D11Device device,
        ID3D11DeviceContext context,
        WinRTDirect3DDevice winrtDevice,
        CaptureNative.MonitorHandle monitor,
        byte[] buffer,
        int vLeft, int vTop, int vWidth)
    {
        var item = CreateItemForMonitor(monitor.Handle);
        var size = item.Size;
        if (size.Width <= 0 || size.Height <= 0)
            return;

        using var framePool = Direct3D11CaptureFramePool.CreateFreeThreaded(
            winrtDevice, DirectXPixelFormat.B8G8R8A8UIntNormalized, 2, size);
        using var session = framePool.CreateCaptureSession(item);

        // Spec: the cursor must not appear in the frame. Border-removal is Win11-only.
        TrySet(() => session.IsCursorCaptureEnabled = false);
        if (OperatingSystem.IsWindowsVersionAtLeast(10, 0, 22000))
            TrySet(() => session.IsBorderRequired = false);

        Direct3D11CaptureFrame? frame = null;
        using var frameReady = new ManualResetEventSlim(false);

        void OnFrameArrived(Direct3D11CaptureFramePool pool, object _)
        {
            frame ??= pool.TryGetNextFrame();
            frameReady.Set();
        }

        framePool.FrameArrived += OnFrameArrived;
        try
        {
            session.StartCapture();
            if (!frameReady.Wait(FirstFrameTimeout) || frame is null)
                throw new TimeoutException($"Timed out capturing monitor at ({monitor.Bounds.Left},{monitor.Bounds.Top}).");

            CopyFrameIntoBuffer(device, context, frame, buffer, monitor, vLeft, vTop, vWidth);
        }
        finally
        {
            framePool.FrameArrived -= OnFrameArrived;
            frame?.Dispose();
        }
    }

    internal static GraphicsCaptureItem CreateItemForMonitor(IntPtr hmon)
    {
        var factory = ActivationFactory.Get("Windows.Graphics.Capture.GraphicsCaptureItem");
        var interop = factory.AsInterface<CaptureNative.IGraphicsCaptureItemInterop>();
        var iid = CaptureNative.GraphicsCaptureItem;
        var itemPtr = interop.CreateForMonitor(hmon, ref iid);
        try
        {
            return GraphicsCaptureItem.FromAbi(itemPtr);
        }
        finally
        {
            Marshal.Release(itemPtr);
        }
    }

    internal static GraphicsCaptureItem CreateItemForWindow(IntPtr hwnd)
    {
        var factory = ActivationFactory.Get("Windows.Graphics.Capture.GraphicsCaptureItem");
        var interop = factory.AsInterface<CaptureNative.IGraphicsCaptureItemInterop>();
        var iid = CaptureNative.GraphicsCaptureItem;
        var itemPtr = interop.CreateForWindow(hwnd, ref iid);
        try
        {
            return GraphicsCaptureItem.FromAbi(itemPtr);
        }
        finally
        {
            Marshal.Release(itemPtr);
        }
    }

    /// <summary>
    /// Fallback when monitor capture fails (typical for exclusive-fullscreen games):
    /// capture the foreground HWND and blit it into the monitor's slot if it overlaps.
    /// </summary>
    private static bool TryCaptureForegroundWindow(
        ID3D11Device device,
        ID3D11DeviceContext context,
        WinRTDirect3DDevice winrtDevice,
        IntPtr hwnd,
        CaptureNative.MonitorHandle monitor,
        byte[] buffer,
        int vLeft, int vTop, int vWidth)
    {
        if (!CaptureNative.GetWindowRect(hwnd, out var wnd) ||
            wnd.Width <= 0 || wnd.Height <= 0)
            return false;

        // Only use the window if it actually covers this monitor (fullscreen / borderless).
        var overlapW = Math.Min(wnd.Right, monitor.Bounds.Right) - Math.Max(wnd.Left, monitor.Bounds.Left);
        var overlapH = Math.Min(wnd.Bottom, monitor.Bounds.Bottom) - Math.Max(wnd.Top, monitor.Bounds.Top);
        if (overlapW < monitor.Bounds.Width / 2 || overlapH < monitor.Bounds.Height / 2)
            return false;

        GraphicsCaptureItem item;
        try
        {
            item = CreateItemForWindow(hwnd);
        }
        catch (Exception ex)
        {
            Log.Warn($"CreateForWindow failed: {ex.Message}");
            return false;
        }

        var size = item.Size;
        if (size.Width <= 0 || size.Height <= 0)
            return false;

        using var framePool = Direct3D11CaptureFramePool.CreateFreeThreaded(
            winrtDevice, DirectXPixelFormat.B8G8R8A8UIntNormalized, 2, size);
        using var session = framePool.CreateCaptureSession(item);
        TrySet(() => session.IsCursorCaptureEnabled = false);
        if (OperatingSystem.IsWindowsVersionAtLeast(10, 0, 22000))
            TrySet(() => session.IsBorderRequired = false);

        Direct3D11CaptureFrame? frame = null;
        using var frameReady = new ManualResetEventSlim(false);

        void OnFrameArrived(Direct3D11CaptureFramePool pool, object _)
        {
            frame ??= pool.TryGetNextFrame();
            frameReady.Set();
        }

        framePool.FrameArrived += OnFrameArrived;
        try
        {
            session.StartCapture();
            if (!frameReady.Wait(FirstFrameTimeout) || frame is null)
                return false;

            // Treat the window as covering this monitor's rect in the virtual desktop.
            var fakeMonitor = new CaptureNative.MonitorHandle(
                monitor.Handle,
                new CaptureNative.RECT
                {
                    Left = monitor.Bounds.Left,
                    Top = monitor.Bounds.Top,
                    Right = monitor.Bounds.Right,
                    Bottom = monitor.Bounds.Bottom,
                },
                monitor.IsPrimary);
            CopyFrameIntoBuffer(device, context, frame, buffer, fakeMonitor, vLeft, vTop, vWidth);
            Log.Info($"Capture: foreground window fallback for monitor ({monitor.Bounds.Left},{monitor.Bounds.Top}).");
            return true;
        }
        finally
        {
            framePool.FrameArrived -= OnFrameArrived;
            frame?.Dispose();
        }
    }

    private static void CopyFrameIntoBuffer(
        ID3D11Device device,
        ID3D11DeviceContext context,
        Direct3D11CaptureFrame frame,
        byte[] buffer,
        CaptureNative.MonitorHandle monitor,
        int vLeft, int vTop, int vWidth)
    {
        var access = frame.Surface.As<CaptureNative.IDirect3DDxgiInterfaceAccess>();
        var iidTex = CaptureNative.ID3D11Texture2D;
        var texPtr = access.GetInterface(ref iidTex);

        using var sourceTex = new ID3D11Texture2D(texPtr);
        var desc = sourceTex.Description;

        var stagingDesc = new Texture2DDescription
        {
            Width = desc.Width,
            Height = desc.Height,
            MipLevels = 1,
            ArraySize = 1,
            Format = desc.Format,
            SampleDescription = new SampleDescription(1, 0),
            Usage = ResourceUsage.Staging,
            BindFlags = BindFlags.None,
            CPUAccessFlags = CpuAccessFlags.Read,
            MiscFlags = ResourceOptionFlags.None,
        };

        using var staging = device.CreateTexture2D(stagingDesc);
        context.CopyResource(staging, sourceTex);

        var map = context.Map(staging, 0, MapMode.Read, Vortice.Direct3D11.MapFlags.None);
        try
        {
            // The captured frame can be a touch larger than the monitor rect; clamp.
            var copyW = Math.Min((int)desc.Width, monitor.Bounds.Width);
            var copyH = Math.Min((int)desc.Height, monitor.Bounds.Height);
            var offsetX = monitor.Bounds.Left - vLeft;
            var offsetY = monitor.Bounds.Top - vTop;
            var rowBytes = copyW * 4;

            var destinationOffset = (offsetY * vWidth + offsetX) * 4;
            CaptureNative.CopyMappedRows(
                map.DataPointer,
                checked((int)map.RowPitch),
                buffer,
                destinationOffset,
                vWidth * 4,
                rowBytes,
                copyH);
        }
        finally
        {
            context.Unmap(staging, 0);
        }
    }

    private static void TrySet(Action set)
    {
        try { set(); }
        catch { /* property unavailable on this OS build; ignore */ }
    }

    private static bool _borderlessRequested;

    /// <summary>
    /// Asks Windows once (Win11) for borderless-capture access, so IsBorderRequired=false is
    /// honoured and the yellow capture border isn't drawn. Fire-and-forget; the grant persists.
    /// </summary>
    public static void RequestBorderlessAccess()
    {
        if (_borderlessRequested || !OperatingSystem.IsWindowsVersionAtLeast(10, 0, 22000))
            return;
        _borderlessRequested = true;
        try
        {
            _ = GraphicsCaptureAccess.RequestAccessAsync(GraphicsCaptureAccessKind.Borderless).AsTask();
        }
        catch (Exception ex)
        {
            Log.Warn($"WGC borderless access request failed: {ex.Message}");
        }
    }
}
