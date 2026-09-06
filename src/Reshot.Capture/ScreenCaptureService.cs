using Reshot.Core.Diagnostics;
using Reshot.Capture.Interop;

namespace Reshot.Capture;

/// <summary>
/// The capture service the app talks to. Snapshots go through Desktop Duplication when the
/// machine allows it and through Windows.Graphics.Capture when it does not; recording is
/// always WGC.
///
/// The split is not about speed. A WGC snapshot has to ask the compositor to leave the
/// cursor out of the frame, and that request pushes the cursor off its hardware plane for
/// as long as the session lives — a visible blink on every screenshot. A duplicated desktop
/// simply never contains the cursor, so the same frame costs nothing to look at. WGC keeps
/// the cases duplication cannot serve: a display driven by another adapter, an
/// exclusive-fullscreen game, a rotated or HDR output.
/// </summary>
public sealed class ScreenCaptureService : IScreenCaptureService
{
    private readonly WgcScreenCaptureService _wgc = new();

    /// <summary>
    /// Set once duplication turns out to be structurally impossible on this machine —
    /// a second adapter driving a display, or a rotated one. That verdict will not change
    /// while the app runs, and re-testing it would cost a device creation per screenshot.
    /// Transient failures (a game holding the display) are not remembered.
    /// </summary>
    private bool _duplicationUnsupported;

    public CapturedFrame SnapshotAllMonitors()
    {
        // Keep one monitor snapshot across the preferred backend and its fallback. Apart
        // from avoiding duplicate user32 work, this makes both backends compose against
        // the exact same virtual-desktop layout during one hotkey capture.
        var monitors = CaptureNative.EnumerateMonitors();
        if (!_duplicationUnsupported)
        {
            try
            {
                return DesktopDuplicationCapture.SnapshotAllMonitors(monitors);
            }
            catch (NotSupportedException ex)
            {
                _duplicationUnsupported = true;
                Log.Warn($"Desktop Duplication unavailable on this display setup ({ex.Message}); " +
                         "using Windows.Graphics.Capture from now on.");
            }
            catch (Exception ex)
            {
                Log.Warn($"Desktop Duplication failed ({ex.Message}); falling back to " +
                         "Windows.Graphics.Capture for this capture.");
            }
        }

        return _wgc.SnapshotAllMonitors(monitors);
    }

    public WgcMonitorStream StartMonitorStream(int screenX, int screenY) =>
        _wgc.StartMonitorStream(screenX, screenY);

    /// <inheritdoc cref="WgcScreenCaptureService.RequestBorderlessAccess"/>
    public static void RequestBorderlessAccess() => WgcScreenCaptureService.RequestBorderlessAccess();
}
