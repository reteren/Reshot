using System.Windows;
using System.Windows.Controls;
using System.Windows.Input;
using System.Windows.Media;
using System.Windows.Media.Animation;
using System.Windows.Shapes;
using Reshot.App.Interop;
// WPF file; disambiguate from the System.Drawing / WinForms types <UseWindowsForms> adds.
using Brushes = System.Windows.Media.Brushes;
using Color = System.Windows.Media.Color;
using Cursors = System.Windows.Input.Cursors;
using Path = System.Windows.Shapes.Path;
using Point = System.Windows.Point;
using Rectangle = System.Windows.Shapes.Rectangle;
using Size = System.Windows.Size;
using ToolTip = System.Windows.Controls.ToolTip;

namespace Reshot.App.Radial;

/// <summary>Which slice of the radial menu was picked.</summary>
public enum RadialChoice { Record, Audio, Settings }

/// <summary>
/// The hold-to-open radial menu: three HL2-styled pie slices (quick record, quick audio,
/// settings) around a central cancel hub, drawn at the cursor.
///
/// By default (<c>radial.clickToChoose</c>) it is a <b>click</b> menu: the wheel outlives
/// the keypress, and a left click commits the slice under the cursor. The central hub means
/// "cancel", which is where the cursor starts, and so does everything past the outer rim.
///
/// Turning that key off makes it a <b>gesture</b> menu instead: the wheel lives only while
/// the hotkey is held and releasing the key commits whatever is hovered. That flips one
/// rule — hover stops being bounded by the rim, because a flick past it should still count
/// as pointing that way. The bound is right for a click, where the pointer lands wherever
/// the user aimed it and a click out on the desktop has to read as "go away" rather than
/// "start recording", and wrong for a flick.
///
/// The window is only as large as the wheel. A topmost window covering the whole screen
/// makes the shell treat it as a fullscreen app and hide the taskbar, which is why this one
/// is sized to the wheel and placed around the (clamped) cursor instead.
/// </summary>
public sealed class RadialMenuWindow : Window
{
    // HL2 / Source palette (base grey = the settings panel grey).
    private static readonly Color Base = Color.FromRgb(0x76, 0x76, 0x76);
    private static readonly Color Hover = Color.FromRgb(0x66, 0x66, 0x66); // slightly darker than base
    private static readonly Color Line = Color.FromRgb(0x88, 0x88, 0x88);  // light-grey outline + cuts
    private static readonly Color Teal = Color.FromRgb(0x3C, 0x98, 0x98);
    private static readonly Color White = Color.FromRgb(0xFF, 0xFF, 0xFF);

    private const double OuterR = 132;
    private const double InnerR = 46;
    private const double ExtendPx = 14;
    private const double GapDeg = 0; // no gaps: the slices tile into one solid circle

    /// <summary>Half the window: the wheel plus room for the hover extend and the stroke.</summary>
    private const double WheelR = OuterR + ExtendPx + 8;

    /// <summary>How often the cursor and the hotkey state are sampled.</summary>
    private const int PollMs = 16;

    /// <summary>Raised after the close animation once a slice was chosen (not on cancel).</summary>
    public event Action<RadialChoice>? Chosen;

    private readonly Canvas _canvas = new();
    private readonly Canvas _wheel = new();
    private readonly ScaleTransform _scale = new(0.85, 0.85);

    /// <summary>Virtual key of the hotkey being held; 0 = no key tracking (click-only).</summary>
    private readonly uint _hotkeyVk;

    /// <summary>Left mouse button, polled the same way the hotkey is.</summary>
    private const int VkLButton = 0x01;

    private System.Windows.Threading.DispatcherTimer? _poll;
    private SolidColorBrush? _hubCross;

    /// <summary>
    /// No key to watch means the wheel has to outlive the keypress, so the mouse commits
    /// instead. The two modes are mutually exclusive by construction.
    /// </summary>
    private bool ClickMode => _hotkeyVk == 0;

    /// <summary>Previous left-button state, so a click commits on the press edge only.</summary>
    private bool _lmbDown;

    /// <summary>Wheel centre in physical pixels, the anchor all hover maths is done against.</summary>
    private int _centerX, _centerY;
    private double _scaleXDpi = 1, _scaleYDpi = 1;

    /// <summary>Hovered slice, or null for the central hub (= cancel).</summary>
    private int? _hover;
    private bool _hovered;

    private sealed class Slice
    {
        public required Path Path { get; init; }
        public required SolidColorBrush Brush { get; init; }
        public required TranslateTransform Extend { get; init; }
        public required double MidRad { get; init; }
        public required RadialChoice Choice { get; init; }
    }

    private readonly List<Slice> _slices = new();
    private RadialChoice? _result;
    private bool _closing;

    // Slice definitions: start angle (screen degrees, 0°=right, CW), label, icon builder.
    private readonly (double Start, RadialChoice Choice, string Name, Func<UIElement> Icon)[] _defs;

    /// <param name="hotkeyVk">
    /// Virtual key currently held down. Releasing it commits the hovered slice. Pass 0 to
    /// disable key tracking, leaving the menu click-driven (<c>radial.clickToChoose</c>).
    /// </param>
    public RadialMenuWindow(uint hotkeyVk = 0)
    {
        _hotkeyVk = hotkeyVk;

        _defs = new (double, RadialChoice, string, Func<UIElement>)[]
        {
            (210, RadialChoice.Record, "Quick screen recording", IconRecord),
            (330, RadialChoice.Audio, "Quick audio recording", IconAudio),
            (90, RadialChoice.Settings, "Settings", IconSettings),
        };

        WindowStyle = WindowStyle.None;
        AllowsTransparency = true;
        Background = Brushes.Transparent;
        ShowInTaskbar = false;
        Topmost = true;
        ResizeMode = ResizeMode.NoResize;
        WindowStartupLocation = WindowStartupLocation.Manual;
        Width = WheelR * 2;
        Height = WheelR * 2;
        Focusable = true;

        _canvas.Children.Add(_wheel);
        Content = _canvas;

        MouseRightButtonDown += (_, _) => Cancel();
        KeyDown += (_, e) => { if (e.Key == Key.Escape) Cancel(); };
        Loaded += OnLoaded;
        Closed += (_, _) => StopPoll();
    }

    /// <summary>
    /// Places the window around the cursor in physical pixels, clamped to the work area so
    /// the wheel never runs off the screen (and never lands under the taskbar).
    /// </summary>
    protected override void OnSourceInitialized(EventArgs e)
    {
        base.OnSourceInitialized(e);

        var dpi = VisualTreeHelper.GetDpi(this);
        _scaleXDpi = dpi.DpiScaleX;
        _scaleYDpi = dpi.DpiScaleY;

        var rx = (int)Math.Round(WheelR * _scaleXDpi);
        var ry = (int)Math.Round(WheelR * _scaleYDpi);

        NativeMethods.GetCursorPos(out var cursor);
        var work = System.Windows.Forms.Screen
            .FromPoint(new System.Drawing.Point(cursor.X, cursor.Y)).WorkingArea;

        _centerX = Clamp(cursor.X, work.Left + rx, work.Right - rx);
        _centerY = Clamp(cursor.Y, work.Top + ry, work.Bottom - ry);

        NativeMethods.SetWindowPos(
            new System.Windows.Interop.WindowInteropHelper(this).Handle,
            NativeMethods.HWND_TOPMOST,
            _centerX - rx, _centerY - ry, rx * 2, ry * 2,
            NativeMethods.SWP_SHOWWINDOW);

        // Keep WPF's own idea of the position in step with the Win32 placement, so a later
        // layout pass can't quietly snap the window back to the Manual default of (0,0).
        Left = (_centerX - rx) / _scaleXDpi;
        Top = (_centerY - ry) / _scaleYDpi;
    }

    /// <summary>Clamps, tolerating a work area smaller than the wheel (then: centred).</summary>
    private static int Clamp(int value, int min, int max) =>
        min > max ? (min + max) / 2 : Math.Clamp(value, min, max);

    private void OnLoaded(object? sender, RoutedEventArgs e)
    {
        Activate();
        Focus();

        // The window is sized to the wheel, so its centre is the wheel's centre.
        var c = new Point(WheelR, WheelR);
        BuildWheel(c);

        // Grow + fade in.
        _wheel.RenderTransform = _scale;
        _scale.CenterX = c.X;
        _scale.CenterY = c.Y;
        _wheel.Opacity = 0;
        var ease = new CubicEase { EasingMode = EasingMode.EaseOut };
        _scale.BeginAnimation(ScaleTransform.ScaleXProperty, new DoubleAnimation(0.85, 1, Secs(0.18)) { EasingFunction = ease });
        _scale.BeginAnimation(ScaleTransform.ScaleYProperty, new DoubleAnimation(0.85, 1, Secs(0.18)) { EasingFunction = ease });
        _wheel.BeginAnimation(OpacityProperty, new DoubleAnimation(0, 1, Secs(0.18)));

        StartPoll();
    }

    // ---- gesture loop ----------------------------------------------------------

    /// <summary>
    /// One timer drives both halves of the gesture: which slice the cursor points at, and
    /// whether the hotkey is still down. Polling the cursor rather than relying on WPF
    /// mouse events is what lets a slice stay selected when the cursor flies past the rim,
    /// well outside this (deliberately small) window.
    /// </summary>
    private void StartPoll()
    {
        // Seed the button state rather than assuming it is up: the menu can open while a
        // button is already held, and a stale "was up" would read that as a fresh click
        // and commit before the user has aimed at anything.
        _lmbDown = IsLeftButtonDown();

        _poll = new System.Windows.Threading.DispatcherTimer
        {
            Interval = TimeSpan.FromMilliseconds(PollMs),
        };
        _poll.Tick += (_, _) =>
        {
            if (_closing)
                return;

            // Hover first, so a release on the very first tick still acts on the direction
            // the cursor is already pointing rather than on a stale "nothing".
            UpdateHoverFromCursor();

            if (ClickMode)
            {
                // The click is polled, not taken from WPF mouse events, for the same reason
                // the cursor is: this window is only as big as the wheel, so a click meant
                // as "cancel" usually lands on another application entirely and never
                // reaches us as an event. Over a fullscreen game the window may not even
                // hold the foreground.
                var down = IsLeftButtonDown();
                var pressed = down && !_lmbDown;
                _lmbDown = down;
                if (pressed)
                    CommitHover();
                return;
            }

            if ((NativeMethods.GetAsyncKeyState((int)_hotkeyVk) & 0x8000) == 0)
                CommitHover(); // key released → act on whatever is under the cursor
        };
        _poll.Start();
    }

    private static bool IsLeftButtonDown() =>
        (NativeMethods.GetAsyncKeyState(VkLButton) & 0x8000) != 0;

    private void StopPoll()
    {
        _poll?.Stop();
        _poll = null;
    }

    private void UpdateHoverFromCursor()
    {
        NativeMethods.GetCursorPos(out var p);
        var dx = (p.X - _centerX) / _scaleXDpi;
        var dy = (p.Y - _centerY) / _scaleYDpi;

        // Inside the hub is the cancel zone, and where the cursor starts. Everything
        // beyond it is a direction; in click mode bounded by the rim, so a click aimed
        // anywhere else on the screen cancels instead of picking the nearest slice, and
        // in gesture mode with no outer limit, so a flick past the rim still counts.
        var dist = Math.Sqrt(dx * dx + dy * dy);
        var outside = dist < InnerR || (ClickMode && dist > OuterR + ExtendPx);
        int? hit = outside ? null : SliceAt(dx, dy);
        SetHover(hit);
    }

    /// <summary>Which slice a direction from the centre falls into.</summary>
    private int? SliceAt(double dx, double dy)
    {
        var deg = Math.Atan2(dy, dx) * 180.0 / Math.PI;
        for (var i = 0; i < _defs.Length; i++)
        {
            var rel = ((deg - _defs[i].Start) % 360 + 360) % 360;
            if (rel < 120)
                return i;
        }
        return null;
    }

    private void SetHover(int? index)
    {
        // _hovered guards the first pass: the hub starts as the hovered target (the cursor
        // opens the wheel at its centre), and "null == null" would skip painting it so.
        if (_closing || (_hovered && _hover == index))
            return;
        _hovered = true;

        if (_hover is int previous)
            HoverSlice(_slices[previous], false);
        _hover = index;
        if (index is int current)
            HoverSlice(_slices[current], true);

        // The hub is "hovered" whenever no slice is, so cancel reads as a real target.
        if (_hubCross is not null)
            AnimateColor(_hubCross, index is null ? Teal : White, 0.15);
    }

    /// <summary>Acts on the hovered slice; the hub (or nothing) cancels.</summary>
    private void CommitHover()
    {
        if (_hover is int i)
            Choose(i);
        else
            Cancel();
    }

    private void BuildWheel(Point c)
    {
        for (int i = 0; i < _defs.Length; i++)
        {
            var def = _defs[i];
            double a0 = Deg2Rad(def.Start + GapDeg);
            double a1 = Deg2Rad(def.Start + 120 - GapDeg);
            double mid = Deg2Rad(def.Start + 60);

            var brush = new SolidColorBrush(Base);
            var extend = new TranslateTransform();
            var path = new Path
            {
                Data = SectorGeometry(c, InnerR, OuterR, a0, a1),
                Fill = brush,
                Stroke = new SolidColorBrush(Line),
                StrokeThickness = 2,
                RenderTransform = extend,
                Cursor = Cursors.Hand,
            };
            ToolTipService.SetInitialShowDelay(path, 1000);
            ToolTipService.SetShowDuration(path, 60000);
            ToolTipService.SetToolTip(path, Hl2Tip(def.Name));

            var slice = new Slice { Path = path, Brush = brush, Extend = extend, MidRad = mid, Choice = def.Choice };
            int index = i;
            // Hover is driven by the cursor poll, not by mouse events; a click is still
            // honoured for anyone who reaches for the mouse out of habit.
            path.MouseLeftButtonDown += (_, _) => Choose(index);

            _slices.Add(slice);
            _wheel.Children.Add(path);

            // Icon centred on the slice's mid-radius, riding the same extend transform.
            var icon = def.Icon();
            icon.IsHitTestVisible = false;
            icon.RenderTransform = extend;
            double ir = (InnerR + OuterR) / 2;
            const double iconSize = 40;
            Canvas.SetLeft(icon, c.X + Math.Cos(mid) * ir - iconSize / 2);
            Canvas.SetTop(icon, c.Y + Math.Sin(mid) * ir - iconSize / 2);
            _wheel.Children.Add(icon);
        }

        // Central cancel hub.
        var hub = new Ellipse
        {
            Width = InnerR * 2, Height = InnerR * 2,
            Fill = new SolidColorBrush(Base),
            Stroke = new SolidColorBrush(Line),
            StrokeThickness = 2,
            Cursor = Cursors.Hand,
        };
        Canvas.SetLeft(hub, c.X - InnerR);
        Canvas.SetTop(hub, c.Y - InnerR);

        double x = 12;
        var xBrush = new SolidColorBrush(White); // teal while the hub is the hovered target
        _hubCross = xBrush;
        var cross = new Path
        {
            Data = Geometry.Parse($"M {c.X - x},{c.Y - x} L {c.X + x},{c.Y + x} M {c.X + x},{c.Y - x} L {c.X - x},{c.Y + x}"),
            Stroke = xBrush,
            StrokeThickness = 4,
            StrokeStartLineCap = PenLineCap.Round,
            StrokeEndLineCap = PenLineCap.Round,
            IsHitTestVisible = false,
        };
        hub.MouseLeftButtonDown += (_, _) => Cancel();
        ToolTipService.SetInitialShowDelay(hub, 1000);
        ToolTipService.SetToolTip(hub, Hl2Tip("Cancel"));

        _wheel.Children.Add(hub);
        _wheel.Children.Add(cross);
    }

    /// <summary>
    /// Hover feedback is deliberately quick: this menu is a flick of the wrist inside a
    /// keypress, so a slow ease would still be catching up when the key comes back up.
    /// </summary>
    private void HoverSlice(Slice s, bool on)
    {
        if (_closing)
            return;
        AnimateColor(s.Brush, on ? Hover : Base, 0.14);
        double tx = on ? Math.Cos(s.MidRad) * ExtendPx : 0;
        double ty = on ? Math.Sin(s.MidRad) * ExtendPx : 0;
        var ease = new CubicEase { EasingMode = EasingMode.EaseOut };
        s.Extend.BeginAnimation(TranslateTransform.XProperty, new DoubleAnimation(tx, Secs(0.14)) { EasingFunction = ease });
        s.Extend.BeginAnimation(TranslateTransform.YProperty, new DoubleAnimation(ty, Secs(0.14)) { EasingFunction = ease });
    }

    private void Choose(int i)
    {
        if (_closing)
            return;
        _closing = true;
        StopPoll();
        _result = _slices[i].Choice;

        // Short teal confirmation, then the close animation. The whole exit is kept under
        // a quarter of a second: the action behind it (record / audio) should feel immediate.
        AnimateColor(_slices[i].Brush, Teal, 0.08);
        var t = new System.Windows.Threading.DispatcherTimer { Interval = Secs(0.12) };
        t.Tick += (_, _) => { t.Stop(); PlayClose(); };
        t.Start();
    }

    private void Cancel()
    {
        if (_closing)
            return;
        _closing = true;
        StopPoll();
        _result = null;
        PlayClose();
    }

    private void PlayClose()
    {
        var ease = new CubicEase { EasingMode = EasingMode.EaseIn };
        var shrink = new DoubleAnimation(1, 0.6, Secs(0.16)) { EasingFunction = ease };
        _scale.BeginAnimation(ScaleTransform.ScaleXProperty, shrink);
        _scale.BeginAnimation(ScaleTransform.ScaleYProperty, shrink);

        var fade = new DoubleAnimation(0, Secs(0.16));
        fade.Completed += (_, _) =>
        {
            var result = _result;
            Close();
            if (result is RadialChoice c)
                Chosen?.Invoke(c);
        };
        _wheel.BeginAnimation(OpacityProperty, fade);
    }

    // ---- helpers ---------------------------------------------------------------

    private static TimeSpan Secs(double s) => TimeSpan.FromSeconds(s);
    private static double Deg2Rad(double d) => d * Math.PI / 180.0;

    private static void AnimateColor(SolidColorBrush brush, Color to, double secs) =>
        brush.BeginAnimation(SolidColorBrush.ColorProperty, new ColorAnimation(to, Secs(secs)));

    private static Geometry SectorGeometry(Point c, double inner, double outer, double a0, double a1)
    {
        Point P(double r, double a) => new(c.X + r * Math.Cos(a), c.Y + r * Math.Sin(a));
        var g = new StreamGeometry();
        using (var ctx = g.Open())
        {
            ctx.BeginFigure(P(inner, a0), isFilled: true, isClosed: true);
            ctx.LineTo(P(outer, a0), true, true);
            ctx.ArcTo(P(outer, a1), new Size(outer, outer), 0, false, SweepDirection.Clockwise, true, true);
            ctx.LineTo(P(inner, a1), true, true);
            ctx.ArcTo(P(inner, a0), new Size(inner, inner), 0, false, SweepDirection.Counterclockwise, true, true);
        }
        g.Freeze();
        return g;
    }

    private static ToolTip Hl2Tip(string text) => new()
    {
        Background = Brushes.Transparent,
        BorderBrush = Brushes.Transparent,
        BorderThickness = new Thickness(0),
        HasDropShadow = false,
        Padding = new Thickness(0),
        Content = new Border
        {
            Background = new SolidColorBrush(Base),
            BorderBrush = new SolidColorBrush(Line),
            BorderThickness = new Thickness(1),
            Padding = new Thickness(9, 5, 9, 5),
            Child = new TextBlock { Text = text, Foreground = Brushes.White, FontSize = 13 },
        },
    };

    // ---- icons (scaled-up hotbar geometry, white) ------------------------------

    private static UIElement IconRecord()
    {
        var canvas = new Canvas { Width = 20, Height = 19 };
        canvas.Children.Add(new Rectangle
        {
            Width = 18, Height = 13, RadiusX = 1.5, RadiusY = 1.5,
            Stroke = Brushes.White, StrokeThickness = 1.6,
            Margin = default,
        });
        var rect = (Rectangle)canvas.Children[0];
        Canvas.SetLeft(rect, 1); Canvas.SetTop(rect, 1);
        var dot = new Ellipse { Width = 4.2, Height = 4.2, Fill = Brushes.White };
        Canvas.SetLeft(dot, 3.6); Canvas.SetTop(dot, 3.6);
        canvas.Children.Add(dot);
        canvas.Children.Add(new Path
        {
            Data = Geometry.Parse("M10,14 L10,17 M5.5,17.5 L14.5,17.5"),
            Stroke = Brushes.White, StrokeThickness = 1.6, StrokeStartLineCap = PenLineCap.Round,
        });
        return Scaled(canvas, 20, 19);
    }

    private static UIElement IconAudio()
    {
        var canvas = new Canvas { Width = 18, Height = 20 };
        var body = new Rectangle
        {
            Width = 7, Height = 11, RadiusX = 3.5, RadiusY = 3.5,
            Stroke = Brushes.White, StrokeThickness = 1.6,
        };
        Canvas.SetLeft(body, 5.5); Canvas.SetTop(body, 1);
        canvas.Children.Add(body);
        canvas.Children.Add(new Path
        {
            Data = Geometry.Parse("M2.5,10 A6.5,6.5 0 0 0 15.5,10"),
            Stroke = Brushes.White, StrokeThickness = 1.6, StrokeStartLineCap = PenLineCap.Round,
        });
        canvas.Children.Add(new Path
        {
            Data = Geometry.Parse("M9,16.5 L9,19"),
            Stroke = Brushes.White, StrokeThickness = 1.6,
        });
        return Scaled(canvas, 18, 20);
    }

    /// <summary>A white gear: eight teeth around a ring, hollow hub.</summary>
    private static UIElement IconSettings()
    {
        const double cx = 10, cy = 10;
        const double rOuter = 9.4, rInner = 6.6, rHub = 2.9;
        const int teeth = 8;
        const double half = 11 * Math.PI / 180; // half tooth width, radians

        var gear = new StreamGeometry();
        using (var ctx = gear.Open())
        {
            Point P(double r, double a) => new(cx + r * Math.Cos(a), cy + r * Math.Sin(a));
            double step = 2 * Math.PI / teeth;

            ctx.BeginFigure(P(rInner, -half - step / 2), isFilled: true, isClosed: true);
            for (int i = 0; i < teeth; i++)
            {
                double c = i * step;
                ctx.LineTo(P(rInner, c - half - step / 4), true, true);
                ctx.LineTo(P(rOuter, c - half), true, true);      // tooth rises
                ctx.LineTo(P(rOuter, c + half), true, true);      // tooth top
                ctx.LineTo(P(rInner, c + half + step / 4), true, true); // back down
            }

            // Hub as a counter-wound circle so it punches a hole (EvenOdd fill).
            ctx.BeginFigure(new Point(cx + rHub, cy), isFilled: true, isClosed: true);
            ctx.ArcTo(new Point(cx - rHub, cy), new Size(rHub, rHub), 0, false, SweepDirection.Clockwise, true, true);
            ctx.ArcTo(new Point(cx + rHub, cy), new Size(rHub, rHub), 0, false, SweepDirection.Clockwise, true, true);
        }
        gear.FillRule = FillRule.EvenOdd;
        gear.Freeze();

        var canvas = new Canvas { Width = 20, Height = 20 };
        canvas.Children.Add(new Path { Data = gear, Fill = Brushes.White });
        return Scaled(canvas, 20, 20);
    }

    private static UIElement Scaled(Canvas content, double w, double h) => new Viewbox
    {
        Width = 40, Height = 40 * h / w,
        Child = content,
    };
}
