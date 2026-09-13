using System.Runtime.InteropServices;
using Reshot.Capture.Interop;
using Reshot.Core.Diagnostics;
using Vortice.Direct3D;
using Vortice.Direct3D11;
using Vortice.DXGI;
using static Vortice.Direct3D11.D3D11;

namespace Reshot.Capture;

/// <summary>
/// Snapshot path built on the DXGI Desktop Duplication API.
///
/// Desktop Duplication normally leaves the cursor on its hardware plane, but a compositor
/// or overlay can bake it into the duplicated pixels. The pointer metadata is authoritative
/// only for acquires that carry a non-zero LastMouseUpdateTime; an unknown state is handed to
/// WGC, whose cursor exclusion switch guarantees a cursor-free still at the cost of a blink.
///
/// It is not a replacement for WGC. Duplication is bound to the adapter that drives the
/// display, gives up in front of an exclusive-fullscreen game, and refuses rotated or
/// non-BGRA outputs. Every one of those cases throws, and the caller falls back.
/// </summary>
internal static class DesktopDuplicationCapture
{
    /// <summary>
    /// Per-acquire wait. Short on purpose: the loop around it is what does the waiting.
    /// </summary>
    private const int AcquireTimeoutMs = 60;

    /// <summary>
    /// How long to keep asking for a frame that actually has desktop content in it. The
    /// first frame after DuplicateOutput is reliably empty — DXGI hands back a surface
    /// before the desktop has been composed into it, and copying that one gives a black
    /// screenshot. Only a frame that accumulated a present is worth reading.
    /// </summary>
    private const int ContentDeadlineMs = 400;

    private static readonly FeatureLevel[] FeatureLevels =
    {
        FeatureLevel.Level_11_1,
        FeatureLevel.Level_11_0,
        FeatureLevel.Level_10_1,
        FeatureLevel.Level_10_0,
    };

    /// <summary>
    /// Grabs every monitor into one virtual-desktop BGRA buffer, or throws so the caller
    /// can fall back. Throwing is a normal outcome here, not a bug.
    /// </summary>
    public static CapturedFrame SnapshotAllMonitors()
        => SnapshotAllMonitors(CaptureNative.EnumerateMonitors());

    internal static CapturedFrame SnapshotAllMonitors(
        IReadOnlyList<CaptureNative.MonitorHandle> monitors)
    {
        var vLeft = CaptureNative.GetSystemMetrics(CaptureNative.SM_XVIRTUALSCREEN);
        var vTop = CaptureNative.GetSystemMetrics(CaptureNative.SM_YVIRTUALSCREEN);
        var vWidth = CaptureNative.GetSystemMetrics(CaptureNative.SM_CXVIRTUALSCREEN);
        var vHeight = CaptureNative.GetSystemMetrics(CaptureNative.SM_CYVIRTUALSCREEN);

        // The monitor list stays the OS one: it carries which display is primary, and it is
        // what the overlay's coordinates are built from. Duplication outputs are matched
        // into it, never used in its place.
        if (monitors.Count == 0)
            throw new InvalidOperationException("No monitors found to capture.");

        using var bufferLease = CapturedFrame.RentBuffer(checked(vWidth * vHeight * 4));
        var buffer = bufferLease.Buffer;
        var captured = new bool[monitors.Count];
        var cursorPositionKnown = CaptureNative.GetCursorPos(out var cursorPosition);

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
        using (var dxgiDevice = device.QueryInterface<IDXGIDevice>())
        using (var adapter = dxgiDevice.GetAdapter())
        {
            for (uint i = 0; ; i++)
            {
                if (adapter.EnumOutputs(i, out var output).Failure || output is null)
                    break;

                using (output)
                {
                    var description = output.Description;
                    if (!description.AttachedToDesktop)
                        continue;

                    var index = IndexOfMonitorAt(
                        monitors, description.DesktopCoordinates.Left, description.DesktopCoordinates.Top);
                    if (index < 0)
                        continue;

                    DuplicateOutputInto(
                        device, context, output, monitors[index], buffer, vLeft, vTop, vWidth,
                        cursorPositionKnown, cursorPosition);
                    captured[index] = true;
                }
            }
        }

        // A monitor on a second adapter is invisible from here. Rather than hand back a
        // frame with a black rectangle in it, give the whole capture to WGC.
        for (var i = 0; i < captured.Length; i++)
        {
            if (!captured[i])
                throw new NotSupportedException(
                    $"Desktop Duplication did not cover the monitor at " +
                    $"({monitors[i].Bounds.Left},{monitors[i].Bounds.Top}).");
        }

        Log.Info($"Capture: {monitors.Count} monitor(s) via Desktop Duplication → " +
                 $"{vWidth}x{vHeight} virtual desktop @ ({vLeft},{vTop}).");

        return bufferLease.CreateFrame(
            vWidth,
            vHeight,
            vLeft,
            vTop,
            monitors
                .Select(m => new CapturedMonitor(
                    m.Bounds.Left, m.Bounds.Top, m.Bounds.Width, m.Bounds.Height, m.IsPrimary))
                .ToList());
    }

    private static int IndexOfMonitorAt(
        IReadOnlyList<CaptureNative.MonitorHandle> monitors, int left, int top)
    {
        for (var i = 0; i < monitors.Count; i++)
        {
            if (monitors[i].Bounds.Left == left && monitors[i].Bounds.Top == top)
                return i;
        }
        return -1;
    }

    private static void DuplicateOutputInto(
        ID3D11Device device,
        ID3D11DeviceContext context,
        IDXGIOutput output,
        CaptureNative.MonitorHandle monitor,
        byte[] buffer,
        int vLeft, int vTop, int vWidth,
        bool cursorPositionKnown,
        CaptureNative.POINT cursorPosition)
    {
        using var output1 = output.QueryInterface<IDXGIOutput1>();

        // Throws on a hybrid-GPU machine where this device is not the one driving the
        // display (DXGI_ERROR_UNSUPPORTED), and on an exclusive-fullscreen app.
        using var duplication = output1.DuplicateOutput(device);

        var deadline = Environment.TickCount64 + ContentDeadlineMs;
        var attempts = 0;
        bool? cursorComposited = null;
        var cursorIsOnMonitor = cursorPositionKnown &&
            cursorPosition.X >= monitor.Bounds.Left &&
            cursorPosition.X < monitor.Bounds.Right &&
            cursorPosition.Y >= monitor.Bounds.Top &&
            cursorPosition.Y < monitor.Bounds.Bottom;

        while (true)
        {
            attempts++;
            var result = duplication.AcquireNextFrame(AcquireTimeoutMs, out var info, out var resource);

            if (result == Vortice.DXGI.ResultCode.WaitTimeout)
            {
                if (Environment.TickCount64 < deadline)
                    continue;
                throw new TimeoutException(
                    $"Desktop Duplication produced no desktop content for the monitor at " +
                    $"({monitor.Bounds.Left},{monitor.Bounds.Top}) in {ContentDeadlineMs} ms.");
            }

            result.CheckError();

            try
            {
                // DXGI only refreshes PointerPosition when the cursor changes. A zero timestamp
                // therefore cannot prove that this frame is cursor-free (especially on the first
                // idle acquire), so retain the last meaningful update and fall back when there
                // has not been one yet. Visible=false means that the cursor was composited into
                // the desktop image; Visible=true means it is a separate pointer plane.
                if (cursorIsOnMonitor && info.LastMouseUpdateTime != 0)
                    cursorComposited = !info.PointerPosition.Visible;

                // AccumulatedFrames == 0 means nothing was presented into this surface: the
                // handshake frame, or a pointer-only update. Its contents are not the
                // desktop, and reading it is exactly how the screenshot comes out black.
                if (info.AccumulatedFrames == 0 && Environment.TickCount64 < deadline)
                    continue;

                if (info.AccumulatedFrames == 0)
                    throw new TimeoutException(
                        $"Desktop Duplication only ever returned empty frames for the monitor " +
                        $"at ({monitor.Bounds.Left},{monitor.Bounds.Top}).");

                if (cursorIsOnMonitor && cursorComposited is not false)
                    throw cursorComposited is true
                        ? new DesktopDuplicationCursorStateException(
                            $"Desktop Duplication reported a composited cursor at " +
                            $"({monitor.Bounds.Left},{monitor.Bounds.Top}).",
                            composited: true)
                        : new DesktopDuplicationCursorStateException(
                            $"Desktop Duplication did not report cursor state for " +
                            $"({monitor.Bounds.Left},{monitor.Bounds.Top}); using WGC.",
                            composited: false);

                if (resource is null)
                    throw new InvalidOperationException("Desktop Duplication returned a null surface.");

                using var sourceTex = resource.QueryInterface<ID3D11Texture2D>();
                CopyIntoBuffer(device, context, sourceTex, monitor, buffer, vLeft, vTop, vWidth);
                // The acquire count is the useful half: it says how many empty frames had
                // to be skipped, so a display that starts needing many of them shows up
                // here before it shows up as a timeout. The pointer state check above is
                // deliberately conservative because idle acquires have no fresh metadata.
                Log.Info($"Duplication: monitor ({monitor.Bounds.Left},{monitor.Bounds.Top}) " +
                         $"took {attempts} acquire(s), {info.AccumulatedFrames} accumulated frame(s).");
                return;
            }
            finally
            {
                resource?.Dispose();
                duplication.ReleaseFrame();
            }
        }
    }

    /// <summary>
    /// Reads the duplicated surface back through a staging texture and blits it into the
    /// monitor's slot of the virtual-desktop buffer.
    /// </summary>
    private static void CopyIntoBuffer(
        ID3D11Device device,
        ID3D11DeviceContext context,
        ID3D11Texture2D sourceTex,
        CaptureNative.MonitorHandle monitor,
        byte[] buffer,
        int vLeft, int vTop, int vWidth)
    {
        var desc = sourceTex.Description;

        // A rotated display arrives rotated, and an HDR one arrives in a wider format.
        // Neither is worth special-casing here when WGC already normalises both.
        if (desc.Format != Format.B8G8R8A8_UNorm)
            throw new NotSupportedException($"Desktop Duplication surface format {desc.Format} is not BGRA8.");
        if (desc.Width != monitor.Bounds.Width || desc.Height != monitor.Bounds.Height)
            throw new NotSupportedException(
                $"Desktop Duplication surface {desc.Width}x{desc.Height} does not match the " +
                $"monitor rect {monitor.Bounds.Width}x{monitor.Bounds.Height} (rotated display?).");

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
            var offsetX = monitor.Bounds.Left - vLeft;
            var offsetY = monitor.Bounds.Top - vTop;
            var rowBytes = monitor.Bounds.Width * 4;

            var destinationOffset = (offsetY * vWidth + offsetX) * 4;
            CaptureNative.CopyMappedRows(
                map.DataPointer,
                checked((int)map.RowPitch),
                buffer,
                destinationOffset,
                vWidth * 4,
                rowBytes,
                monitor.Bounds.Height);
        }
        finally
        {
            context.Unmap(staging, 0);
        }
    }
}

/// <summary>
/// Signals that Desktop Duplication cannot prove this still excludes a cursor. The caller
/// should use WGC for the current capture; this is not a structural duplication failure.
/// </summary>
internal sealed class DesktopDuplicationCursorStateException : Exception
{
    internal DesktopDuplicationCursorStateException(string message, bool composited) : base(message)
    {
        IsComposited = composited;
    }

    internal bool IsComposited { get; }
}
