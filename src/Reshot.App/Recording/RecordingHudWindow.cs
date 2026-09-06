using System.Runtime.InteropServices;
using System.Windows;
using System.Windows.Controls;
using System.Windows.Interop;
using System.Windows.Media;
using System.Windows.Shapes;
using System.Windows.Threading;
using Reshot.Core.Diagnostics;
using Brush = System.Windows.Media.Brush;
using Brushes = System.Windows.Media.Brushes;
using Color = System.Windows.Media.Color;
using Colors = System.Windows.Media.Colors;
using ColorConverter = System.Windows.Media.ColorConverter;
using FontFamily = System.Windows.Media.FontFamily;
using Orientation = System.Windows.Controls.Orientation;
using Rectangle = System.Windows.Shapes.Rectangle;
using Size = System.Windows.Size;

namespace Reshot.App.Recording;

/// <summary>
/// The on-screen recording HUD (SPEC §9): a full-virtual-desktop, click-through,
/// topmost window that draws corner brackets around the recorded area and a small
/// top-right indicator (red dot + elapsed timer + resolution). Purely visual, all
/// mouse input passes through to the live screen beneath.
/// </summary>
public sealed class RecordingHudWindow : Window
{
    private const int GwlExStyle = -20;
    private const int WsExTransparent = 0x20, WsExLayered = 0x80000, WsExToolWindow = 0x80, WsExNoActivate = 0x08000000;

    // Keep this window off every screen-capture API (WGC included). Visible on the
    // physical display; absent from captured frames. Win10 2004+ / build 19041.
    private const uint WdaExcludeFromCapture = 0x00000011;

    [DllImport("user32.dll")] private static extern int GetWindowLong(IntPtr hwnd, int index);
    [DllImport("user32.dll")] private static extern int SetWindowLong(IntPtr hwnd, int index, int value);
    [DllImport("user32.dll")] private static extern bool SetWindowDisplayAffinity(IntPtr hwnd, uint affinity);

    private readonly DispatcherTimer _timer = new() { Interval = TimeSpan.FromMilliseconds(500) };
    private readonly DateTime _start = DateTime.Now;
    private readonly Ellipse _dot;
    private readonly TextBlock _label;
    private readonly string _resolution;
    private bool _blink;
    private int _lastSecond = -1;

    public RecordingHudWindow(
        Int32Rect recordRect, string resolution,
        bool cornersEnabled, string cornerColorHex, double cornerOpacity,
        Int32Rect primaryScreen)
    {
        _resolution = resolution;

        WindowStyle = WindowStyle.None;
        AllowsTransparency = true;
        Background = Brushes.Transparent;
        ResizeMode = ResizeMode.NoResize;
        ShowInTaskbar = false;
        Topmost = true;
        IsHitTestVisible = false;
        Focusable = false;
        ShowActivated = false;

        // Cover the whole virtual desktop (assumes uniform DPI, mixed DPI is out of scope).
        Left = SystemParameters.VirtualScreenLeft;
        Top = SystemParameters.VirtualScreenTop;
        Width = SystemParameters.VirtualScreenWidth;
        Height = SystemParameters.VirtualScreenHeight;

        var canvas = new Canvas();
        Content = canvas;

        var originX = SystemParameters.VirtualScreenLeft;
        var originY = SystemParameters.VirtualScreenTop;

        if (cornersEnabled)
        {
            var color = ParseColor(cornerColorHex);
            var brush = new SolidColorBrush(color) { Opacity = Math.Clamp(cornerOpacity, 0, 1) };
            brush.Freeze();
            DrawCorners(canvas, recordRect.X - originX, recordRect.Y - originY, recordRect.Width, recordRect.Height, brush);
        }

        // Indicator, top-right of the primary monitor.
        _dot = new Ellipse { Width = 11, Height = 11, Fill = Brushes.Red, VerticalAlignment = VerticalAlignment.Center };
        _label = new TextBlock
        {
            Foreground = Brushes.White,
            FontFamily = new FontFamily("Segoe UI"),
            FontSize = 13,
            FontWeight = FontWeights.SemiBold,
            VerticalAlignment = VerticalAlignment.Center,
            Margin = new Thickness(8, 0, 2, 0),
        };
        var panel = new StackPanel { Orientation = Orientation.Horizontal };
        panel.Children.Add(_dot);
        panel.Children.Add(_label);
        var indicator = new Border
        {
            Background = new SolidColorBrush(Color.FromArgb(0xCC, 0x18, 0x18, 0x1B)),
            BorderBrush = new SolidColorBrush(Color.FromArgb(0x66, 0xFF, 0xFF, 0xFF)),
            BorderThickness = new Thickness(1),
            CornerRadius = new CornerRadius(6),
            Padding = new Thickness(10, 6, 12, 6),
            Child = panel,
        };
        indicator.Measure(new Size(400, 100));
        var indicatorWidth = indicator.DesiredSize.Width > 0 ? indicator.DesiredSize.Width : 190;
        Canvas.SetLeft(indicator, primaryScreen.X + primaryScreen.Width - indicatorWidth - 18 - originX);
        Canvas.SetTop(indicator, primaryScreen.Y + 18 - originY);
        canvas.Children.Add(indicator);

        UpdateLabel();
        _timer.Tick += (_, _) =>
        {
            _blink = !_blink;
            _dot.Opacity = _blink ? 0.25 : 1.0;
            UpdateLabel();
        };
        _timer.Start();
    }

    private void UpdateLabel()
    {
        var t = DateTime.Now - _start;
        var totalSec = (int)t.TotalSeconds;
        if (totalSec == _lastSecond)
            return;
        _lastSecond = totalSec;
        _label.Text = $"REC  {(int)t.TotalMinutes:00}:{t.Seconds:00}   ·   {_resolution}";
    }

    private static void DrawCorners(Canvas canvas, double x, double y, double w, double h, System.Windows.Media.Brush brush)
    {
        const double leg = 28, thick = 4;
        void L(double px, double py, double dx, double dy)
        {
            var line = new Rectangle
            {
                Width = dx == 0 ? thick : leg,
                Height = dy == 0 ? thick : leg,
                Fill = brush,
            };
            Canvas.SetLeft(line, px + (dx < 0 ? -leg + thick : 0));
            Canvas.SetTop(line, py + (dy < 0 ? -leg + thick : 0));
            canvas.Children.Add(line);
        }
        // Each corner = one horizontal + one vertical leg.
        L(x, y, 1, 0); L(x, y, 0, 1);                                   // top-left
        L(x + w - leg, y, 1, 0); L(x + w - thick, y, 0, 1);            // top-right
        L(x, y + h - thick, 1, 0); L(x, y + h - leg, 0, 1);           // bottom-left
        L(x + w - leg, y + h - thick, 1, 0); L(x + w - thick, y + h - leg, 0, 1); // bottom-right
    }

    private static System.Windows.Media.Color ParseColor(string hex)
    {
        try
        {
            if (ColorConverter.ConvertFromString(hex) is System.Windows.Media.Color c)
                return c;
        }
        catch { /* fall through */ }
        return Colors.Red;
    }

    protected override void OnSourceInitialized(EventArgs e)
    {
        base.OnSourceInitialized(e);
        var hwnd = new WindowInteropHelper(this).Handle;
        var ex = GetWindowLong(hwnd, GwlExStyle);
        SetWindowLong(hwnd, GwlExStyle, ex | WsExTransparent | WsExLayered | WsExToolWindow | WsExNoActivate);

        // WGC composites this topmost HUD into the captured frame, so without exclusion
        // the corner brackets and REC indicator end up burned into the MP4.
        if (!SetWindowDisplayAffinity(hwnd, WdaExcludeFromCapture))
            Log.Warn("Recording HUD: SetWindowDisplayAffinity(WDA_EXCLUDEFROMCAPTURE) failed; the HUD may appear in the recording.");
    }

    protected override void OnClosed(EventArgs e)
    {
        _timer.Stop();
        base.OnClosed(e);
    }
}
