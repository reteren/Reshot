using System.Runtime.CompilerServices;
using System.Runtime.InteropServices;
using System.Threading;
using Reshot.Capture.Interop;
using Reshot.Core.Diagnostics;
using Vortice.Direct3D;
using Vortice.Direct3D11;
using Vortice.DXGI;
using Windows.Graphics.Capture;
using Windows.Graphics.DirectX;
using WinRT;
using static Vortice.Direct3D11.D3D11;
using WinRTDirect3DDevice = Windows.Graphics.DirectX.Direct3D11.IDirect3DDevice;

[assembly: InternalsVisibleTo("Reshot.Recording.Tests")]

namespace Reshot.Capture;

/// <summary>
/// Continuous Windows.Graphics.Capture of a single monitor for video recording
/// (ARCHITECTURE §9). Keeps only the most recent frame in a monitor-sized BGRA
/// buffer; the recorder pulls a cropped region from it at the target frame rate,
/// which decouples the encode cadence from the display's refresh/dirty rate.
/// </summary>
public sealed class WgcMonitorStream : IDisposable
{
    private readonly ID3D11Device _device;
    private readonly ID3D11DeviceContext _context;
    private readonly WinRTDirect3DDevice _winrtDevice;
    private readonly Direct3D11CaptureFramePool _framePool;
    private readonly GraphicsCaptureSession _session;
    private ID3D11Texture2D? _staging;

    private readonly byte[] _latest;   // monitor-sized BGRA, guarded by _lock
    private readonly object _lock = new();
    private volatile bool _hasFrame;
    private int _isDisposed;
    private int _inFlight;
    private readonly ManualResetEventSlim _drained = new(false);

    internal int InFlightCallbacks => Volatile.Read(ref _inFlight);
    internal bool IsDisposed => Volatile.Read(ref _isDisposed) != 0;
    internal Action? FrameProcessingStarting { get; set; }

    /// <summary>Monitor origin in virtual-screen coordinates.</summary>
    public int MonitorLeft { get; }
    public int MonitorTop { get; }
    public int Width { get; }
    public int Height { get; }

    public WgcMonitorStream(IntPtr hmon, int monLeft, int monTop, int monWidth, int monHeight)
    {
        MonitorLeft = monLeft;
        MonitorTop = monTop;
        Width = monWidth;
        Height = monHeight;
        _latest = new byte[(long)monWidth * monHeight * 4];

        ID3D11Device? device = null;
        ID3D11DeviceContext? context = null;
        WinRTDirect3DDevice? winrtDevice = null;
        Direct3D11CaptureFramePool? framePool = null;
        GraphicsCaptureSession? session = null;
        var subscribed = false;

        try
        {
            D3D11CreateDevice(
                null, DriverType.Hardware, DeviceCreationFlags.BgraSupport,
                WgcScreenCaptureService.FeatureLevels, out var createdDevice).CheckError();
            device = createdDevice ?? throw new InvalidOperationException("D3D11CreateDevice returned null.");
            context = device.ImmediateContext;
            winrtDevice = WgcScreenCaptureService.CreateWinRtDevice(device);

            var item = WgcScreenCaptureService.CreateItemForMonitor(hmon);
            var size = item.Size;
            framePool = Direct3D11CaptureFramePool.CreateFreeThreaded(
                winrtDevice, DirectXPixelFormat.B8G8R8A8UIntNormalized, 2, size);
            session = framePool.CreateCaptureSession(item);

            // Assign fields before StartCapture: the free-threaded callback may arrive immediately.
            _device = device;
            _context = context;
            _winrtDevice = winrtDevice;
            _framePool = framePool;
            _session = session;

            // Screen recordings keep the cursor visible (unlike the still snapshot).
            // Hide the yellow Windows capture border (Win11 only; needs the SDK 22000+ API).
            WgcScreenCaptureService.RequestBorderlessAccess();
            if (OperatingSystem.IsWindowsVersionAtLeast(10, 0, 22000))
                TrySet(() => _session.IsBorderRequired = false);

            _framePool.FrameArrived += OnFrameArrived;
            subscribed = true;
            _session.StartCapture();
            Log.Info($"Stream: capturing monitor {monWidth}x{monHeight} @ ({monLeft},{monTop}).");
        }
        catch
        {
            if (subscribed && framePool is not null)
                framePool.FrameArrived -= OnFrameArrived;
            session?.Dispose();
            framePool?.Dispose();
            (winrtDevice as IDisposable)?.Dispose();
            context?.Dispose();
            device?.Dispose();
            _drained.Dispose();
            throw;
        }
    }

    private void OnFrameArrived(Direct3D11CaptureFramePool pool, object _)
    {
        Interlocked.Increment(ref _inFlight);
        if (Volatile.Read(ref _isDisposed) != 0)
        {
            if (Interlocked.Decrement(ref _inFlight) == 0 && Volatile.Read(ref _isDisposed) != 0)
                _drained.Set();
            return;
        }

        try
        {
            FrameProcessingStarting?.Invoke();

            Direct3D11CaptureFrame? frame = null;
            try
            {
                frame = pool.TryGetNextFrame();
            }
            catch (Exception ex) when (ex is ObjectDisposedException or COMException)
            {
                return;
            }

            if (frame is null || Volatile.Read(ref _isDisposed) != 0)
            {
                frame?.Dispose();
                return;
            }

            using (frame)
            {
                var access = frame.Surface.As<CaptureNative.IDirect3DDxgiInterfaceAccess>();
                var iidTex = CaptureNative.ID3D11Texture2D;
                var texPtr = access.GetInterface(ref iidTex);
                using var sourceTex = new ID3D11Texture2D(texPtr);
                var desc = sourceTex.Description;

                if (Volatile.Read(ref _isDisposed) != 0)
                    return;

                if (_staging is null || _staging.Description.Width != desc.Width || _staging.Description.Height != desc.Height)
                {
                    _staging?.Dispose();
                    _staging = _device.CreateTexture2D(new Texture2DDescription
                    {
                        Width = desc.Width,
                        Height = desc.Height,
                        MipLevels = 1,
                        ArraySize = 1,
                        Format = desc.Format,
                        SampleDescription = new SampleDescription(1, 0),
                        Usage = ResourceUsage.Staging,
                        CPUAccessFlags = CpuAccessFlags.Read,
                    });
                }

                _context.CopyResource(_staging, sourceTex);
                var map = _context.Map(_staging, 0, MapMode.Read, Vortice.Direct3D11.MapFlags.None);
                try
                {
                    var copyW = Math.Min((int)desc.Width, Width);
                    var copyH = Math.Min((int)desc.Height, Height);
                    var rowBytes = copyW * 4;
                    var destStride = Width * 4;
                    lock (_lock)
                    {
                        CaptureNative.CopyMappedRows(
                            map.DataPointer,
                            checked((int)map.RowPitch),
                            _latest,
                            0,
                            destStride,
                            rowBytes,
                            copyH);
                        _hasFrame = true;
                    }
                }
                finally
                {
                    _context.Unmap(_staging, 0);
                }
            }
        }
        catch (Exception ex)
        {
            if (Volatile.Read(ref _isDisposed) == 0)
                Log.Warn($"Stream: frame processing error: {ex.Message}");
        }
        finally
        {
            if (Interlocked.Decrement(ref _inFlight) == 0 && Volatile.Read(ref _isDisposed) != 0)
            {
                _drained.Set();
            }
        }
    }

    /// <summary>
    /// Copies a crop (rect relative to the monitor's top-left, clamped) of the latest
    /// frame into <paramref name="dest"/> (top-down BGRA, stride cropWidth*4). Returns
    /// false if no frame has arrived yet.
    /// </summary>
    public bool CopyRegion(int cropLeft, int cropTop, int cropWidth, int cropHeight, byte[] dest)
    {
        if (!_hasFrame)
            return false;

        var srcStride = Width * 4;
        var dstStride = cropWidth * 4;
        lock (_lock)
        {
            for (var y = 0; y < cropHeight; y++)
                Buffer.BlockCopy(_latest, (cropTop + y) * srcStride + cropLeft * 4, dest, y * dstStride, dstStride);
        }
        return true;
    }

    private static void TrySet(Action set)
    {
        try { set(); }
        catch { /* property unavailable on this OS build */ }
    }

    public void Dispose()
    {
        if (Interlocked.Exchange(ref _isDisposed, 1) != 0)
            return;

        _framePool.FrameArrived -= OnFrameArrived;
        _session.Dispose();
        _framePool.Dispose();

        // Drain any currently executing frame callback before releasing D3D resources.
        if (Volatile.Read(ref _inFlight) > 0)
        {
            _drained.Wait(TimeSpan.FromSeconds(2));
        }

        _staging?.Dispose();
        (_winrtDevice as IDisposable)?.Dispose();
        _context.Dispose();
        _device.Dispose();
        _drained.Dispose();
        Log.Info("Stream: stopped.");
    }

    internal void ProcessFrameForTest(Direct3D11CaptureFramePool pool) => OnFrameArrived(pool, null!);
}
