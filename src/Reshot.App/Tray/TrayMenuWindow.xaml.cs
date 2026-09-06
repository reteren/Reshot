using System.Windows;
using System.Windows.Input;
using System.Windows.Interop;
using System.Windows.Media;
using Reshot.App.Interop;

namespace Reshot.App.Tray;

/// <summary>
/// The tray context menu, drawn as a borderless window instead of a
/// <c>ContextMenuStrip</c> so it can carry the same Half-Life 2 styling as the
/// settings dialog. Raises intent events; it holds no app logic itself.
/// </summary>
/// <remarks>
/// The panel used to ask DWM for blur-behind (<c>SetWindowCompositionAttribute</c> with
/// <c>ACCENT_ENABLE_BLURBEHIND</c>) to echo the settings modal's backdrop-filter. On a
/// layered window — which <c>AllowsTransparency</c> makes this one — DWM paints that
/// accent across the whole window rectangle, including the pixels the rounded corners
/// leave empty, and a zero gradient colour renders them black. The result was four black
/// wedges around the panel. The translucency lives in the panel brush itself now, so the
/// corners are simply transparent.
/// </remarks>
public partial class TrayMenuWindow : Window
{
    private bool _closing;

    public event EventHandler? CaptureRequested;
    public event EventHandler? SettingsRequested;
    public event EventHandler? QuitRequested;

    /// <summary>Fires when the user toggles "Pause hotkey"; arg = paused.</summary>
    public event EventHandler<bool>? PauseHotkeyToggled;

    public TrayMenuWindow(bool paused)
    {
        InitializeComponent();

        WindowStartupLocation = WindowStartupLocation.Manual;
        PauseCheck.IsChecked = paused;

        CaptureBtn.Click += (_, _) => Fire(CaptureRequested);
        SettingsBtn.Click += (_, _) => Fire(SettingsRequested);
        QuitBtn.Click += (_, _) => Fire(QuitRequested);

        PauseCheck.Click += (_, _) =>
        {
            var paused = PauseCheck.IsChecked == true;
            CloseOnce();
            PauseHotkeyToggled?.Invoke(this, paused);
        };

        // Clicking anywhere else dismisses the menu, like a real context menu.
        Deactivated += (_, _) => CloseOnce();
    }

    /// <summary>
    /// Closes at most once. Closing the window also deactivates it, which re-enters
    /// the Deactivated handler, and WPF throws if <c>Close</c> is called while a
    /// close is already in progress.
    /// </summary>
    private void CloseOnce()
    {
        if (_closing)
            return;

        _closing = true;
        if (Mouse.Captured is not null && IsDescendant(Mouse.Captured as DependencyObject))
        {
            Mouse.Capture(null);
        }
        Close();
    }

    protected override void OnPreviewMouseDown(MouseButtonEventArgs e)
    {
        base.OnPreviewMouseDown(e);
        var pos = e.GetPosition(this);
        if (pos.X < 0 || pos.X > ActualWidth || pos.Y < 0 || pos.Y > ActualHeight)
        {
            CloseOnce();
        }
    }

    protected override void OnLostMouseCapture(MouseEventArgs e)
    {
        base.OnLostMouseCapture(e);
        if (_closing)
            return;

        var captured = Mouse.Captured as DependencyObject;
        if (captured is null || !IsDescendant(captured))
        {
            CloseOnce();
        }
    }

    protected override void OnPreviewKeyDown(KeyEventArgs e)
    {
        base.OnPreviewKeyDown(e);
        if (e.Key == Key.Escape)
        {
            CloseOnce();
            e.Handled = true;
        }
    }

    private bool IsDescendant(DependencyObject? element)
    {
        while (element is not null)
        {
            if (ReferenceEquals(element, this))
                return true;

            DependencyObject? parent = null;
            if (element is Visual or System.Windows.Media.Media3D.Visual3D)
            {
                parent = VisualTreeHelper.GetParent(element);
            }
            parent ??= LogicalTreeHelper.GetParent(element);
            element = parent;
        }
        return false;
    }

    protected override void OnClosing(System.ComponentModel.CancelEventArgs e)
    {
        _closing = true;
        base.OnClosing(e);
    }

    protected override void OnClosed(EventArgs e)
    {
        Content = null;
        base.OnClosed(e);
    }

    private void Fire(EventHandler? handler)
    {
        CloseOnce();
        handler?.Invoke(this, EventArgs.Empty);
    }

    /// <summary>
    /// Shows the menu near the mouse, kept inside the work area of the screen it
    /// was summoned on. The tray lives in a corner, so the menu normally opens
    /// up and to the left of the cursor.
    /// </summary>
    public void ShowAtCursor()
    {
        NativeMethods.GetCursorPos(out var cursor);
        NativeMethods.GetDpiForPoint(cursor, out var dpiScaleX, out var dpiScaleY);
        var work = NativeMethods.GetWorkingArea(cursor);

        // Pre-position on the target monitor before Show() to prevent flashing at (0,0)
        // and to ensure WPF queries monitor DPI for the target monitor rather than the primary.
        var cursorDipX = cursor.X / dpiScaleX;
        var cursorDipY = cursor.Y / dpiScaleY;
        Left = cursorDipX;
        Top = cursorDipY;

        var helper = new WindowInteropHelper(this);
        var hwnd = helper.EnsureHandle();

        // Win32 notify-icon rule: assert foreground on the menu HWND before showing/activating,
        // defeating foreground lock arbitration so Deactivated and focus behave normally.
        NativeMethods.ForceForegroundWindow(hwnd, invasive: false);

        // Measure before positioning: SizeToContent means the size is unknown
        // until the layout pass runs.
        Show();
        UpdateLayout();

        // Convert target monitor bounds to DIPs using that specific monitor's DPI scale.
        var workLeft = work.Left / dpiScaleX;
        var workTop = work.Top / dpiScaleY;
        var workRight = work.Right / dpiScaleX;
        var workBottom = work.Bottom / dpiScaleY;

        // Prefer opening up-left of the cursor; flip to the other side when the
        // tray sits at the top or left of the screen instead.
        var left = cursorDipX - ActualWidth;
        if (left < workLeft)
            left = cursorDipX;

        var top = cursorDipY - ActualHeight;
        if (top < workTop)
            top = cursorDipY;

        Left = Math.Min(left, workRight - ActualWidth);
        Top = Math.Min(top, workBottom - ActualHeight);

        Activate();

        // Fallback dismissal: capture mouse within this visual subtree so any outside click
        // dismisses the menu even if Windows foreground arbitration failed.
        Mouse.Capture(this, CaptureMode.SubTree);
    }
}
