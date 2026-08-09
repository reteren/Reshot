using System.Runtime.InteropServices;
using System.Windows;
using System.Windows.Controls;
using System.Windows.Input;
using System.Windows.Interop;
using System.Windows.Media;
using System.Windows.Media.Animation;
using System.Windows.Media.Imaging;
using System.Windows.Shapes;
using System.Windows.Threading;
using Reshot.App.Export;
using Reshot.App.Interop;
using Reshot.App.Ocr;
using Reshot.Capture;
using Reshot.Core.Diagnostics;
using Reshot.Core.Document;
using Reshot.Core.History;
using Reshot.Core.Settings;
using Reshot.Core.Tools;
using SkiaSharp;
using SkiaSharp.Views.Desktop;
// This is a WPF file; disambiguate from the WinForms/System.Drawing implicit usings
// that <UseWindowsForms> adds project-wide (needed only by the tray icon).
using Brush = System.Windows.Media.Brush;
using Brushes = System.Windows.Media.Brushes;
using Button = System.Windows.Controls.Button;
using Color = System.Windows.Media.Color;
using ColorConverter = System.Windows.Media.ColorConverter;
using Cursor = System.Windows.Input.Cursor;
using Cursors = System.Windows.Input.Cursors;
using KeyEventArgs = System.Windows.Input.KeyEventArgs;
using MenuItem = System.Windows.Controls.MenuItem;
using MouseEventArgs = System.Windows.Input.MouseEventArgs;
using TextBox = System.Windows.Controls.TextBox;
using Point = System.Windows.Point;
using Rectangle = System.Windows.Shapes.Rectangle;
using Size = System.Windows.Size;

namespace Reshot.App.Overlay;

/// <summary>
/// Full virtual-desktop overlay: shows the frozen frame dimmed, lets the user
/// select an area with any shape (SPEC §4), edit its bounding box, and export it
/// with transparency outside the shape. Positioned in physical pixels so
/// multi-monitor + negative origins line up with the capture buffer.
/// </summary>
public partial class OverlayWindow : Window
{
    private readonly CapturedFrame _frame;
    private readonly AppSettings _settings;
    private readonly CaptureExporter _exporter;

    private BitmapSource? _frameBitmap;
    private Selection? _selection;                       // the active (editable) selection
    private readonly List<Selection> _committed = new(); // additional Ctrl+drag selections
    private ShapeKind _activeShape = ShapeKind.Rectangle;

    // Drawing (Phase 3): the editable document + brush.
    private CaptureDocument? _document;
    private readonly UndoHistory _history = new();
    private ToolMode _tool = ToolMode.Select;
    private DrawSubTool _drawSub = DrawSubTool.Brush;

    // Per-tool settings: every tool keeps its own size / opacity / colour, so switching
    // tools no longer carries the previous tool's values over. Effects store their Strength
    // (0..1) in the Opacity field. _brush always resolves to the active tool's settings.
    private readonly Dictionary<ToolSettingsKey, BrushSettings> _toolSettings = new()
    {
        [ToolSettingsKey.Brush]    = new() { Thickness = 5f,  Opacity = 1f,    Color = new(0xFF, 0x3B, 0x30) },
        [ToolSettingsKey.Shapes]   = new() { Thickness = 5f,  Opacity = 1f,    Color = new(0xFF, 0x3B, 0x30) },
        [ToolSettingsKey.Lines]    = new() { Thickness = 5f,  Opacity = 1f,    Color = new(0xFF, 0x3B, 0x30) },
        [ToolSettingsKey.Text]     = new() { Thickness = 5f,  Opacity = 1f,    Color = new(0xFF, 0x3B, 0x30) },
        [ToolSettingsKey.Eraser]   = new() { Thickness = 25f, Opacity = 1f },
        [ToolSettingsKey.Pixelize] = new() { Thickness = 30f, Opacity = 0.15f },
        [ToolSettingsKey.Blur]     = new() { Thickness = 30f, Opacity = 0.50f },
    };
    private BrushSettings _brush => _toolSettings[ToolKeyFor(_drawSub)];
    private BrushStroke? _stroke;
    private SKPath? _strokeClip;
    private VectorObject? _vectorPreview;   // shape being dragged
    private SKPoint _shapeStart;
    private VectorObject? _textEditing;      // text being typed
    private Dictionary<DrawSubTool, Button> _toolButtons = new();
    private Dictionary<ShapeKind, Button> _shapeButtons = new();
    private Border? _openFlyout;
    private DispatcherTimer? _hoverTimer;
    private EventHandler? _hoverTick;
    private DispatcherTimer? _flyoutCloseTimer;
    private EventHandler? _flyoutCloseTick;
    private bool _openedByHover;
    private bool _panelAnimating;
    private readonly Dictionary<Border, Button> _flyoutOwner = new();

    // Each category remembers its last sub-tool, so clicking the tab resumes it.
    // The initial values are the first entry of every flyout (the HUD's defaults).
    private DrawSubTool _shapesSub = DrawSubTool.Square;
    private DrawSubTool _linesSub = DrawSubTool.Arrow;
    private DrawSubTool _eraserSub = DrawSubTool.Eraser;
    private DrawSubTool _effectsSub = DrawSubTool.Pixelize;

    // OCR: recognise text in the selection and make it selectable/copyable.
    private OcrTextLayer? _ocr;
    private bool _ocrMode;
    private bool _ocrSelecting;
    private bool _ocrBusy;
    private string _ocrLang = "auto"; // "auto" merges RU+EN per line; or force "ru" / "en".

    // Effects (blur/pixelize).
    private SKBitmap? _baseBitmap;
    private SKBitmap? _blurBase;
    private SKBitmap? _pixelBase;
    private float _blurSigma = -1f;    // strength the cached _blurBase was built with
    private int _pixelFactor = -1;     // strength the cached _pixelBase was built with
    private SKShader? _checkerShader;  // transparency checker for absolute-erased areas
    private SKRect _absoluteBounds = SKRect.Empty; // union of committed absolute-erase areas
    private SKPath? _effectStroke;    // freehand effect brush
    private SKPoint _effectStart;
    private bool _effectRectMode;     // Ctrl held → rectangle area
    private SKRect _effectRect;
    private const float EffectBrushWidth = 46f;
    private bool _brushPanelOpen;
    private bool _eyedropping;

    /// <summary>
    /// Eyedropper armed from the panel button: unlike the RMB-hold shortcut it stays live
    /// (previewing under the cursor) until the user right-clicks to take the colour.
    /// </summary>
    private bool _eyedropperArmed;
    private SKPoint _cursorPhysical;
    private bool _brushCursorVisible;

    // HSV picker state.
    private double _hue;   // 0..360
    private double _sat;   // 0..1
    private double _val;   // 0..1
    private const double SvW = 204, SvH = 130, HueH = 130;

    private enum ToolMode { Select, Draw }

    private enum DrawSubTool
    {
        Brush, Line, Arrow, Square, Circle, Triangle, Text,
        Blur, Pixelize,
        Eraser, AbsoluteEraser, FilterEraser,
    }

    private static bool IsRegionTool(DrawSubTool t) => t is
        DrawSubTool.Blur or DrawSubTool.Pixelize or
        DrawSubTool.Eraser or DrawSubTool.AbsoluteEraser or DrawSubTool.FilterEraser;

    // Which settings slot a sub-tool draws from. Shapes share one slot, lines share one,
    // all three erasers share one; blur and pixelize each get their own (different defaults).
    private enum ToolSettingsKey { Brush, Shapes, Lines, Text, Eraser, Blur, Pixelize }

    private static ToolSettingsKey ToolKeyFor(DrawSubTool sub) => sub switch
    {
        DrawSubTool.Square or DrawSubTool.Circle or DrawSubTool.Triangle => ToolSettingsKey.Shapes,
        DrawSubTool.Line or DrawSubTool.Arrow => ToolSettingsKey.Lines,
        DrawSubTool.Text => ToolSettingsKey.Text,
        DrawSubTool.Eraser or DrawSubTool.AbsoluteEraser or DrawSubTool.FilterEraser => ToolSettingsKey.Eraser,
        DrawSubTool.Blur => ToolSettingsKey.Blur,
        DrawSubTool.Pixelize => ToolSettingsKey.Pixelize,
        _ => ToolSettingsKey.Brush,
    };

    // Interaction state for the current mouse gesture.
    private DragMode _dragMode = DragMode.None;
    private Handle _activeHandle = Handle.None;
    private Point _dragStart;      // gesture anchor (DIP)
    private Rect _refRect;         // bounds at gesture start (for move/resize)
    private bool _shiftAtStart;    // Shift held when the gesture began → 1:1
    private double? _lockedRatio;  // ratio locked when Shift pressed mid-gesture
    private CtrlAMode _ctrlAMode = CtrlAMode.None;
    private readonly List<Point> _freehandPoints = new(); // lasso in-progress (DIP)

    // Polygon click-mode state (SPEC §4.2, tool 1.3).
    private bool _polygonInProgress;
    private readonly List<Point> _polygonPoints = new();
    private Point _polygonCursor;
    private const double PolygonCloseThreshold = 12;

    // Keep this window off every screen-capture API (WGC included). Visible on the
    // physical display; absent from captured frames. Win10 2004+ / build 19041.
    private const uint WdaExcludeFromCapture = 0x00000011;

    [DllImport("user32.dll")] private static extern bool SetWindowDisplayAffinity(IntPtr hwnd, uint affinity);

    private const double HandleSize = 9;
    private readonly Dictionary<Handle, Rectangle> _handleShapes = new();

    private double _dpiScaleX = 1.0;
    private double _dpiScaleY = 1.0;

    private bool _selectionActive;

    /// <summary>Raised when the session ends. True if a result was produced.</summary>
    public event EventHandler<bool>? SessionEnded;

    /// <summary>Raised when a selection appears or disappears (drives the session FSM).</summary>
    public event Action<bool>? SelectionActiveChanged;

    /// <summary>
    /// Raised when Record is pressed. Carries the recording rect in virtual-screen
    /// pixels and an optional coverage mask (stride = rect.Width, 0 = outside the
    /// selected shape(s)); null when the selection is a single full-rect.
    /// </summary>
    public event Action<Int32Rect, byte[]?, Reshot.Recording.AudioSources?>? RecordRequested;

    /// <summary>Raised when audio recording is started (left-click = defaults, menu = chosen sources).</summary>
    public event Action<Reshot.Recording.AudioSources>? AudioRecordRequested;

    private enum DragMode { None, NewSelection, Move, Resize }

    private enum Handle { None, TopLeft, Top, TopRight, Right, BottomRight, Bottom, BottomLeft, Left }

    private enum CtrlAMode { None, Primary, All }

    public OverlayWindow(CapturedFrame frame, AppSettings settings)
    {
        _frame = frame;
        _settings = settings;
        _exporter = new CaptureExporter(settings);
        InitializeComponent();

        SourceInitialized += OnSourceInitialized;
        Loaded += OnLoaded;
    }

    /// <summary>The current selection's bounds in physical (capture-buffer) pixels.</summary>
    public Int32Rect? SelectionInPixels
    {
        get
        {
            if (_selection is not { } sel)
                return null;

            var b = sel.Bounds;
            var x = (int)Math.Round(b.X * _dpiScaleX);
            var y = (int)Math.Round(b.Y * _dpiScaleY);
            var w = (int)Math.Round(b.Width * _dpiScaleX);
            var h = (int)Math.Round(b.Height * _dpiScaleY);
            return new Int32Rect(x, y, Math.Max(0, w), Math.Max(0, h));
        }
    }

    private void OnSourceInitialized(object? sender, EventArgs e)
    {
        var dpi = VisualTreeHelper.GetDpi(this);
        _dpiScaleX = dpi.DpiScaleX;
        _dpiScaleY = dpi.DpiScaleY;

        var hwnd = new WindowInteropHelper(this).Handle;
        NativeMethods.SetWindowPos(
            hwnd, NativeMethods.HWND_TOPMOST,
            _frame.VirtualLeft, _frame.VirtualTop, _frame.Width, _frame.Height,
            NativeMethods.SWP_SHOWWINDOW);

        // This overlay is a fullscreen topmost window, and the recording stream starts
        // before it closes, so without exclusion the frozen frame, selection outline and
        // toolbar end up burned into the first frames of the MP4.
        if (!SetWindowDisplayAffinity(hwnd, WdaExcludeFromCapture))
            Log.Warn("Overlay: SetWindowDisplayAffinity(WDA_EXCLUDEFROMCAPTURE) failed; the overlay may appear in the recording.");
    }

    private void OnLoaded(object? sender, RoutedEventArgs e)
    {
        _frameBitmap = FrameImage.ToBitmapSource(_frame);
        FrameView.Source = _frameBitmap;
        DimPath.Fill = BuildDimBrush();

        _document = new CaptureDocument(_frame.Width, _frame.Height);
        DrawSurface.PaintSurface += OnPaintSurface;

        CopyButton.Click += (_, _) => ExportAndClose(ExportKind.Copy);
        SaveButton.Click += (_, _) => ExportAndClose(ExportKind.Save);
        SaveAsButton.Click += (_, _) => ExportAndClose(ExportKind.SaveAs);
        CloseButton.Click += (_, _) => EndSession(produced: false);

        RecordTool.Click += (_, _) => RequestRecording();
        RecordTool.MouseRightButtonUp += (_, e) => { e.Handled = true; ShowVideoAudioMenu(); };
        AudioTool.Click += (_, _) => StartAudioWithDefaults();
        AudioTool.MouseRightButtonUp += (_, e) => { e.Handled = true; ShowAudioMenu(); };

        _toolButtons = new Dictionary<DrawSubTool, Button>
        {
            [DrawSubTool.Brush] = ToolBrushBtn,
            [DrawSubTool.Line] = ToolLineBtn,
            [DrawSubTool.Arrow] = ToolArrowBtn,
            [DrawSubTool.Square] = ToolSquareBtn,
            [DrawSubTool.Circle] = ToolCircleBtn,
            [DrawSubTool.Triangle] = ToolTriangleBtn,
            [DrawSubTool.Text] = ToolTextBtn,
            [DrawSubTool.Blur] = ToolBlurBtn,
            [DrawSubTool.Pixelize] = ToolPixelizeBtn,
            [DrawSubTool.Eraser] = ToolEraserBtn,
            [DrawSubTool.AbsoluteEraser] = ToolAbsEraserBtn,
            [DrawSubTool.FilterEraser] = ToolFilterEraserBtn,
        };
        foreach (var (sub, btn) in _toolButtons)
            btn.Click += (_, _) => { CloseFlyouts(); SetDrawSub(sub); };

        WireBrushPanel();

        // Selection shapes live in their own flyout off the Select tab.
        _shapeButtons = new Dictionary<ShapeKind, Button>
        {
            [ShapeKind.Rectangle] = ShapeRectBtn,
            [ShapeKind.Ellipse] = ShapeEllipseBtn,
            [ShapeKind.Lasso] = ShapeLassoBtn,
            [ShapeKind.Polygon] = ShapePolygonBtn,
            [ShapeKind.Triangle] = ShapeTriangleBtn,
        };
        foreach (var (kind, btn) in _shapeButtons)
            btn.Click += (_, _) => { CloseFlyouts(); SetActiveShape(kind); SetTool(ToolMode.Select); };

        // A tab activates its category (resuming that category's last sub-tool);
        // right-clicking it reveals the sub-tools, per the HUD spec.
        WireTab(SelectTab, SelectFlyout, () => SetTool(ToolMode.Select));
        WireTab(ShapesTab, ShapesFlyout, () => SetDrawSub(_shapesSub));
        WireTab(LinesTab, LinesFlyout, () => SetDrawSub(_linesSub));
        WireTab(EraserTab, EraserFlyout, () => SetDrawSub(_eraserSub));
        WireTab(EffectsTab, EffectsFlyout, () => SetDrawSub(_effectsSub));

        // OCR tab: left-click recognises text in the selection; right-click flips RU/EN.
        OcrTab.Click += (_, _) => { CloseFlyouts(); RunOcr(); };
        OcrTab.MouseRightButtonUp += (_, e) => { e.Handled = true; ToggleOcrLanguage(); };

        // The strip above the toolbar is the mouse-driven twin of the Shift key.
        SettingsStrip.MouseLeftButtonUp += (_, e) => { e.Handled = true; CloseFlyouts(); ToggleBrushPanel(); };
        SettingsStrip.MouseEnter += (_, _) => StripHover.Opacity = 0.5;
        SettingsStrip.MouseLeave += (_, _) => StripHover.Opacity = 0;

        CreateHandles();
        SetActiveShape(_activeShape);
        HighlightTool();
        UpdateVisuals();
        ForceForeground();
    }

    /// <summary>
    /// Left-click runs the tab's action; right-click toggles its sub-tools, and
    /// simply resting on the tab for a moment reveals them too.
    /// </summary>
    private void WireTab(Button tab, Border? flyout, Action activate)
    {
        tab.Click += (_, _) => { CloseFlyouts(); activate(); };

        if (flyout is null)
            return;

        tab.MouseRightButtonUp += (_, e) =>
        {
            e.Handled = true;
            _hoverTimer?.Stop();
            var reopen = flyout.Visibility != Visibility.Visible;
            CloseFlyouts();
            if (reopen)
                ShowFlyout(flyout, tab);
        };

        _flyoutOwner[flyout] = tab;

        tab.MouseEnter += (_, _) =>
        {
            // Returning to the tab that owns the open flyout cancels its dismissal;
            // arriving at any other tab must not keep a stale one alive.
            if (ReferenceEquals(_openFlyout, flyout))
                _flyoutCloseTimer?.Stop();
            else
                ArmFlyoutClose();

            ArmHover(flyout, tab);
        };
        tab.MouseLeave += (_, _) => { _hoverTimer?.Stop(); ArmFlyoutClose(); };
        flyout.MouseEnter += (_, _) => _flyoutCloseTimer?.Stop();
        flyout.MouseLeave += (_, _) => ArmFlyoutClose();
    }

    /// <summary>
    /// Dismisses the open flyout once the pointer has left both it and its tab. The
    /// delay covers the gap the pointer crosses on the way from one to the other.
    /// </summary>
    private void ArmFlyoutClose()
    {
        // A flyout summoned with the right button stays until it is dismissed
        // deliberately; only the hover shortcut times out.
        if (_openFlyout is null || !_openedByHover)
            return;

        _flyoutCloseTimer ??= new DispatcherTimer { Interval = TimeSpan.FromMilliseconds(450) };
        _flyoutCloseTimer.Stop();
        _flyoutCloseTimer.Tick -= _flyoutCloseTick;
        _flyoutCloseTick = (_, _) =>
        {
            _flyoutCloseTimer!.Stop();
            if (_openFlyout is not { } open)
                return;
            if (open.IsMouseOver || (_flyoutOwner.TryGetValue(open, out var owner) && owner.IsMouseOver))
                return;
            CloseFlyouts();
        };
        _flyoutCloseTimer.Tick += _flyoutCloseTick;
        _flyoutCloseTimer.Start();
    }

    /// <summary>Opens <paramref name="flyout"/> once the pointer has rested on its tab.</summary>
    private void ArmHover(Border flyout, Button tab)
    {
        if (flyout.Visibility == Visibility.Visible)
            return;

        _hoverTimer ??= new DispatcherTimer { Interval = TimeSpan.FromSeconds(0.5) };
        _hoverTimer.Stop();

        _hoverTimer.Tick -= _hoverTick;
        _hoverTick = (_, _) =>
        {
            _hoverTimer!.Stop();
            // The pointer may have moved on, or a right-click may have opened it
            // already, during the delay.
            if (tab.IsMouseOver && flyout.Visibility != Visibility.Visible)
                ShowFlyout(flyout, tab, byHover: true);
        };
        _hoverTimer.Tick += _hoverTick;
        _hoverTimer.Start();
    }

    private void ShowFlyout(Border flyout, Button anchor, bool byHover = false)
    {
        // Only ever one flyout on screen, whichever way it was summoned.
        if (_openFlyout is { } other && !ReferenceEquals(other, flyout))
            CloseFlyouts();

        CloseBrushPanel();
        _hoverTimer?.Stop();
        flyout.Visibility = Visibility.Visible;
        _openFlyout = flyout;
        _openedByHover = byHover;
        PositionFlyout(flyout, anchor);

        // Rise into place out of the tab it belongs to.
        var slide = EnsureSlide(flyout);
        flyout.Opacity = 0;
        slide.Y = 14;
        Animate(flyout, OpacityProperty, 1);
        Animate(slide, TranslateTransform.YProperty, 0);
    }

    private void CloseFlyouts()
    {
        foreach (var f in AllFlyouts())
        {
            if (f.Visibility != Visibility.Visible)
                continue;

            var slide = EnsureSlide(f);
            var closing = f;
            Animate(f, OpacityProperty, 0, () =>
            {
                // Skip the hide if the flyout was reopened mid-animation.
                if (!ReferenceEquals(_openFlyout, closing))
                    closing.Visibility = Visibility.Collapsed;
            });
            Animate(slide, TranslateTransform.YProperty, 14);
        }
        _openFlyout = null;
        _flyoutCloseTimer?.Stop();
    }

    private static TranslateTransform EnsureSlide(Border flyout)
    {
        if (flyout.RenderTransform is TranslateTransform t)
            return t;

        var slide = new TranslateTransform();
        flyout.RenderTransform = slide;
        return slide;
    }

    private IEnumerable<Border> AllFlyouts()
    {
        yield return SelectFlyout;
        yield return ShapesFlyout;
        yield return LinesFlyout;
        yield return EraserFlyout;
        yield return EffectsFlyout;
    }

    /// <summary>Centres a flyout over its tab, above the toolbar, clamped on-screen.</summary>
    private void PositionFlyout(Border flyout, Button anchor)
    {
        flyout.UpdateLayout();
        Toolbar.UpdateLayout();

        var toolbarLeft = Canvas.GetLeft(Toolbar);
        var toolbarTop = Canvas.GetTop(Toolbar);
        if (double.IsNaN(toolbarLeft)) toolbarLeft = 0;
        if (double.IsNaN(toolbarTop)) toolbarTop = 0;

        // Anchor's offset within the toolbar, so the flyout tracks the actual tab.
        var offset = anchor.TranslatePoint(new Point(0, 0), Toolbar);
        var x = toolbarLeft + offset.X + (anchor.ActualWidth - flyout.ActualWidth) / 2;

        var y = toolbarTop - flyout.ActualHeight - 4;
        if (SettingsStrip.Visibility == Visibility.Visible)
            y -= SettingsStrip.ActualHeight + 3;
        if (y < 0)
            y = toolbarTop + Toolbar.ActualHeight + 4;

        x = Math.Clamp(x, 0, Math.Max(0, RootGrid.ActualWidth - flyout.ActualWidth));
        y = Math.Clamp(y, 0, Math.Max(0, RootGrid.ActualHeight - flyout.ActualHeight));
        Canvas.SetLeft(flyout, x);
        Canvas.SetTop(flyout, y);
    }

    private void SetActiveShape(ShapeKind kind)
    {
        _activeShape = kind;
        foreach (var (k, btn) in _shapeButtons)
            btn.Tag = k == kind ? "on" : null;
        Log.Info($"Selection shape → {kind}.");

        // Leaving polygon mode mid-draw cancels the in-progress outline.
        if (kind != ShapeKind.Polygon && _polygonInProgress)
            CancelPolygon();

        // Convert an existing box-shaped selection in place (rectangle ⇄ ellipse ⇄ triangle).
        if (_selection is { IsFreeform: false } s && kind is not ShapeKind.Lasso and not ShapeKind.Polygon)
        {
            s.Kind = kind;
            s.NormalizedPoints = null;
            UpdateVisuals();
        }
    }

    /// <summary>Creates the 8 resize handles once; they are repositioned on each update.</summary>
    private void CreateHandles()
    {
        foreach (Handle h in Enum.GetValues<Handle>())
        {
            if (h == Handle.None)
                continue;

            var shape = new Rectangle
            {
                Width = HandleSize,
                Height = HandleSize,
                Fill = new SolidColorBrush(Color.FromRgb(0xFF, 0xFF, 0xFF)),
                Stroke = new SolidColorBrush(Color.FromRgb(0x3C, 0x98, 0x98)),
                StrokeThickness = 1.5,
                Visibility = Visibility.Collapsed,
            };
            _handleShapes[h] = shape;
            ChromeLayer.Children.Add(shape);
        }
    }

    /// <summary>
    /// Takes keyboard focus for the overlay. Only the WPF half lives here: the Win32 side
    /// of getting in front of a game (unlocking the pointer, stealing the foreground,
    /// holding topmost) belongs to <c>App</c>, which knows whether a game is involved at
    /// all. Doing it here as well ran the whole dance twice per capture, Alt trick
    /// included, which the user saw as the cursor blinking.
    /// </summary>
    private void ForceForeground()
    {
        Activate();
        Focus();
        Keyboard.Focus(this);
    }

    private Brush BuildDimBrush()
    {
        var color = ParseColor(_settings.Dim.Color, fallback: Colors.Black);
        var opacity = Math.Clamp(_settings.Dim.Opacity, 0.0, 1.0);
        color.A = (byte)Math.Round(opacity * 255);
        var brush = new SolidColorBrush(color);
        brush.Freeze();
        return brush;
    }

    private static Color ParseColor(string hex, Color fallback)
    {
        try
        {
            return (Color)ColorConverter.ConvertFromString(hex);
        }
        catch
        {
            return fallback;
        }
    }

    // ---- Mouse -----------------------------------------------------------------

    protected override void OnMouseLeftButtonDown(MouseButtonEventArgs e)
    {
        base.OnMouseLeftButtonDown(e);

        // HUD panels own their clicks. Buttons mark the event handled themselves, but
        // plain surfaces (the settings strip, the panel's backdrop) do not, without
        // this the window would start a stroke and capture the mouse, so the strip
        // never saw its own MouseUp.
        if (IsOverChrome())
            return;

        var p = e.GetPosition(RootGrid);

        // OCR mode: clicks select recognised text instead of drawing/selecting.
        if (_ocrMode)
        {
            OcrMouseDown(p);
            return;
        }

        // Polygon click-mode: each click adds a vertex (double-click / near-first closes).
        if (_polygonInProgress)
        {
            HandlePolygonClick(p, e.ClickCount);
            return;
        }

        // Draw tool: brush / shape / text instead of selecting.
        if (_tool == ToolMode.Draw)
        {
            if (_drawSub == DrawSubTool.Brush)
                BeginBrush(p);
            else if (_drawSub == DrawSubTool.Text)
                BeginText(p);
            else if (IsRegionTool(_drawSub))
                BeginEffect(p);
            else
                BeginShape(p);
            return;
        }

        // Editing the active selection (handle → resize, body → move) beats a new one.
        if (_selection is { Bounds.Width: > 0, Bounds.Height: > 0 } sel)
        {
            var handle = HitTestHandle(p, sel.Bounds);
            if (handle != Handle.None)
            {
                PrepGesture(p);
                _dragMode = DragMode.Resize;
                _activeHandle = handle;
                _refRect = sel.Bounds;
                CaptureMouse();
                UpdateVisuals();
                return;
            }
            // Move only when grabbing the frame's edge line (not the interior), so a
            // new selection can be started inside an existing one.
            if (IsOnEdge(sel.Bounds, p))
            {
                PrepGesture(p);
                _dragMode = DragMode.Move;
                _refRect = sel.Bounds;
                CaptureMouse();
                UpdateVisuals();
                return;
            }
        }

        // Grabbing a committed selection's edge makes it the active one.
        var committedIndex = HitCommitted(p);
        if (committedIndex >= 0)
        {
            var picked = _committed[committedIndex];
            _committed.RemoveAt(committedIndex);
            if (_selection is { Bounds.Width: > 0, Bounds.Height: > 0 })
                _committed.Add(_selection);
            _selection = picked;

            PrepGesture(p);
            _dragMode = DragMode.Move;
            _refRect = picked.Bounds;
            CaptureMouse();
            UpdateVisuals();
            return;
        }

        // Empty space: with Ctrl, keep the current selection as another region.
        var additive = Keyboard.Modifiers.HasFlag(ModifierKeys.Control);
        CommitActiveIfAdditive(additive);

        if (_activeShape == ShapeKind.Polygon)
        {
            StartPolygon(p);
            return;
        }

        PrepGesture(p);
        BeginDragSelection(p);
        CaptureMouse();
        UpdateVisuals();
    }

    /// <summary>Pushes the active selection into the committed set (Ctrl), or clears all.</summary>
    private void CommitActiveIfAdditive(bool additive)
    {
        if (additive)
        {
            if (_selection is { Bounds.Width: > 0, Bounds.Height: > 0 })
                _committed.Add(_selection);
        }
        else
        {
            _committed.Clear();
        }
        _selection = null;
    }

    /// <summary>Index of the top-most committed selection whose frame edge is under p, else -1.</summary>
    private int HitCommitted(Point p)
    {
        for (var i = _committed.Count - 1; i >= 0; i--)
        {
            if (IsOnEdge(_committed[i].Bounds, p))
                return i;
        }
        return -1;
    }

    // ---- Tools / drawing -------------------------------------------------------

    private void SetTool(ToolMode tool)
    {
        ExitOcrMode(); // picking any drawing/selection tool leaves OCR text mode
        CommitText();
        _tool = tool;
        Cursor = Cursors.Cross;
        if (tool == ToolMode.Select)
        {
            _brushCursorVisible = false;
            CloseBrushPanel();
            CloseFlyouts();
        }
        HighlightTool();
        Log.Info($"Tool → {tool}.");
        RefreshDrawSurface();
    }

    /// <summary>Enters Draw mode on the brush (the "2" shortcut).</summary>
    private void EnterDrawMode() => SetDrawSub(DrawSubTool.Brush);

    /// <summary>Maps a number key to its 1-based toolbar slot (0 = the tenth), or null.</summary>
    private static int? ToolSlot(Key key) => key switch
    {
        Key.D1 or Key.NumPad1 => 1,
        Key.D2 or Key.NumPad2 => 2,
        Key.D3 or Key.NumPad3 => 3,
        Key.D4 or Key.NumPad4 => 4,
        Key.D5 or Key.NumPad5 => 5,
        Key.D6 or Key.NumPad6 => 6,
        Key.D7 or Key.NumPad7 => 7,
        Key.D8 or Key.NumPad8 => 8,
        Key.D9 or Key.NumPad9 => 9,
        Key.D0 or Key.NumPad0 => 10,
        _ => null,
    };

    /// <summary>
    /// Activates a toolbar slot exactly as clicking its tab would: categories resume their
    /// last sub-tool, and the action tools (OCR, record) run their action.
    /// </summary>
    private void ActivateToolSlot(int slot)
    {
        CloseFlyouts();
        switch (slot)
        {
            case 1: SetTool(ToolMode.Select); break;
            case 2: SetDrawSub(DrawSubTool.Brush); break;
            case 3: SetDrawSub(_shapesSub); break;
            case 4: SetDrawSub(_linesSub); break;
            case 5: SetDrawSub(_eraserSub); break;
            case 6: SetDrawSub(_effectsSub); break;
            case 7: RunOcr(); break;
            case 8: SetDrawSub(DrawSubTool.Text); break;
            case 9: RequestRecording(); break;
            case 10: StartAudioWithDefaults(); break;
        }
    }

    private void SetDrawSub(DrawSubTool sub)
    {
        CommitText();

        // Picking a different tool folds the settings panel away, its contents
        // belong to the tool that was active when it was opened.
        if (sub != _drawSub)
            CloseBrushPanel();

        _drawSub = sub;

        // Remember the choice so the category's tab resumes it next time.
        switch (Category(sub))
        {
            case ToolCategory.Shapes: _shapesSub = sub; break;
            case ToolCategory.Lines: _linesSub = sub; break;
            case ToolCategory.Eraser: _eraserSub = sub; break;
            case ToolCategory.Effects: _effectsSub = sub; break;
        }

        SetTool(ToolMode.Draw);
        HighlightTool();
        LoadToolSettingsIntoPanel();
        Cursor = sub == DrawSubTool.Text ? Cursors.IBeam : Cursors.Cross;
        Log.Info($"Draw sub-tool → {sub}.");
    }

    private enum ToolCategory { Brush, Shapes, Lines, Eraser, Effects, Text }

    private static ToolCategory Category(DrawSubTool sub) => sub switch
    {
        DrawSubTool.Square or DrawSubTool.Circle or DrawSubTool.Triangle => ToolCategory.Shapes,
        DrawSubTool.Line or DrawSubTool.Arrow => ToolCategory.Lines,
        DrawSubTool.Eraser or DrawSubTool.AbsoluteEraser or DrawSubTool.FilterEraser => ToolCategory.Eraser,
        DrawSubTool.Blur or DrawSubTool.Pixelize => ToolCategory.Effects,
        DrawSubTool.Text => ToolCategory.Text,
        _ => ToolCategory.Brush,
    };

    private void HighlightTool()
    {
        // Brush and Text are their own tabs, so the dictionary covers them too.
        foreach (var (sub, btn) in _toolButtons)
            btn.Tag = !_ocrMode && _tool == ToolMode.Draw && sub == _drawSub ? "on" : null;

        var drawing = !_ocrMode && _tool == ToolMode.Draw;
        var category = Category(_drawSub);
        SelectTab.Tag = !_ocrMode && _tool == ToolMode.Select ? "on" : null;
        ShapesTab.Tag = drawing && category == ToolCategory.Shapes ? "on" : null;
        LinesTab.Tag = drawing && category == ToolCategory.Lines ? "on" : null;
        EraserTab.Tag = drawing && category == ToolCategory.Eraser ? "on" : null;
        EffectsTab.Tag = drawing && category == ToolCategory.Effects ? "on" : null;
        OcrTab.Tag = _ocrMode ? "on" : null;

        UpdateToolSettingsUi();
    }

    /// <summary>
    /// Pulls the active tool's saved size / opacity(strength) / colour into the panel
    /// controls, so each tool shows and edits its own settings. Called on every switch.
    /// </summary>
    private void LoadToolSettingsIntoPanel()
    {
        var s = _brush;
        ThicknessSlider.Value = s.Thickness;
        ThicknessValue.Text = $"{(int)Math.Round(s.Thickness)} px";
        OpacitySlider.Value = s.Opacity * 100.0;
        OpacityValue.Text = $"{(int)Math.Round(s.Opacity * 100.0)}%";
        HardnessSlider.Value = s.Hardness * 100.0;
        HardnessValue.Text = $"{(int)Math.Round(s.Hardness * 100.0)}%";
        SetBrushColor(s.Color);
    }

    /// <summary>Colour applies to ink tools only; erasers and effects have none.</summary>
    private static bool HasColor(DrawSubTool sub) =>
        Category(sub) is ToolCategory.Brush or ToolCategory.Shapes or ToolCategory.Lines or ToolCategory.Text;

    /// <summary>
    /// Shows the settings strip for tools that have adjustable settings and tailors
    /// the panel to the active one: effects trade Opacity for Strength (it drives
    /// blur sigma / pixel size), and erasers only have a size.
    /// </summary>
    private void UpdateToolSettingsUi()
    {
        if (_ocrMode)
        {
            SettingsStrip.Visibility = Visibility.Collapsed;
            CloseBrushPanel();
            return;
        }

        var drawing = _tool == ToolMode.Draw;
        SettingsStrip.Visibility = drawing && Toolbar.Visibility == Visibility.Visible
            ? Visibility.Visible
            : Visibility.Collapsed;

        if (!drawing)
        {
            CloseBrushPanel();
            return;
        }

        var category = Category(_drawSub);
        var eraser = category == ToolCategory.Eraser;
        ColorSection.Visibility = HasColor(_drawSub) ? Visibility.Visible : Visibility.Collapsed;
        // Erasers hide the plain strength row and instead show the Photoshop-hardness
        // "Opacity" row (soft radial falloff). Other tools keep the strength row.
        StrengthRow.Visibility = eraser ? Visibility.Collapsed : Visibility.Visible;
        StrengthLabel.Text = category == ToolCategory.Effects ? "Strength" : "Opacity";
        HardnessRow.Visibility = eraser ? Visibility.Visible : Visibility.Collapsed;

        // The strip is shown from here (tool switches) as well as from PositionToolbar
        // (selection changes), so it has to lay itself out on both paths.
        LayoutSettingsStrip();

        if (_brushPanelOpen)
            PositionBrushPanel();
    }

    /// <summary>
    /// Parks the strip directly above the toolbar. It is the collapsed form of the
    /// settings panel, so it takes the panel's width rather than the toolbar's.
    /// </summary>
    private void LayoutSettingsStrip()
    {
        if (SettingsStrip.Visibility != Visibility.Visible)
            return;

        Toolbar.UpdateLayout();
        var toolbarX = Canvas.GetLeft(Toolbar);
        var toolbarY = Canvas.GetTop(Toolbar);
        if (double.IsNaN(toolbarX) || double.IsNaN(toolbarY))
            return;

        SettingsStrip.Width = BrushPanel.Width;

        var y = toolbarY - SettingsStrip.Height - 3;
        if (y < 0)
            y = toolbarY + Toolbar.ActualHeight + 3;

        var x = Math.Clamp(toolbarX, 0, Math.Max(0, RootGrid.ActualWidth - SettingsStrip.Width));
        Canvas.SetLeft(SettingsStrip, x);
        Canvas.SetTop(SettingsStrip, y);
    }

    // ---- Shapes & text ---------------------------------------------------------

    private static VectorKind MapShape(DrawSubTool sub) => sub switch
    {
        DrawSubTool.Line => VectorKind.Line,
        DrawSubTool.Arrow => VectorKind.Arrow,
        DrawSubTool.Square => VectorKind.Square,
        DrawSubTool.Circle => VectorKind.Circle,
        DrawSubTool.Triangle => VectorKind.Triangle,
        _ => VectorKind.Square,
    };

    private void BeginShape(Point p)
    {
        if (!AllSelections().Any())
            return;

        _shapeStart = ToSkPhysical(p);
        _vectorPreview = new VectorObject
        {
            Kind = MapShape(_drawSub),
            Color = _brush.Color,
            Thickness = _brush.Thickness,
            P1 = _shapeStart,
            P2 = _shapeStart,
            Bounds = SKRect.Create(_shapeStart.X, _shapeStart.Y, 0, 0),
        };
        CaptureMouse();
    }

    private void UpdateShape(SKPoint cur)
    {
        if (_vectorPreview is null)
            return;

        if (_vectorPreview.Kind is VectorKind.Line or VectorKind.Arrow)
        {
            _vectorPreview.P2 = cur;
        }
        else
        {
            _vectorPreview.Bounds = SKRect.Create(
                Math.Min(_shapeStart.X, cur.X), Math.Min(_shapeStart.Y, cur.Y),
                Math.Abs(cur.X - _shapeStart.X), Math.Abs(cur.Y - _shapeStart.Y));

            // Triangle points the way you drag (up when dragging upward).
            if (_vectorPreview.Kind == VectorKind.Triangle)
                _vectorPreview.PointsUp = cur.Y <= _shapeStart.Y;
        }
    }

    private void EndShape()
    {
        ReleaseMouseCapture();
        if (_vectorPreview is { } v && _document is not null)
        {
            var b = v.ComputeBounds();
            if (b.Width >= 3 || b.Height >= 3)
                CommitVectorToPaint(v);
        }
        _vectorPreview = null;
        RefreshDrawSurface();
    }

    /// <summary>
    /// Bakes a finished shape/text into the paint layer, clipped to the selection
    /// exactly like a brush stroke, with a region-snapshot undo. Once baked it is
    /// ordinary ink: the eraser removes it pixel-by-pixel, never as a whole object.
    /// </summary>
    private void CommitVectorToPaint(VectorObject v)
    {
        if (_document is null)
            return;

        var region = VectorRegion(v);
        if (region is not { Width: > 0, Height: > 0 })
            return;

        using var clip = BuildSelectionClip();
        var before = CaptureDocument.SnapshotRegion(_document.PaintLayer, region);
        _document.CommitVector(v, clip);
        var after = CaptureDocument.SnapshotRegion(_document.PaintLayer, region);
        _history.Push(new LayerRegionCommand(_document.PaintLayer, region, before, after));
    }

    /// <summary>Paint region a shape/text touches, padded for stroke width and arrowheads.</summary>
    private SKRectI VectorRegion(VectorObject v)
    {
        // 44px covers the largest arrowhead barb (clamped to 40 in VectorObject) plus AA.
        var pad = v.Thickness / 2f + 44f;
        return RectToRegion(SKRect.Inflate(v.ComputeBounds(), pad, pad));
    }

    private void BeginText(Point p)
    {
        if (!AllSelections().Any())
            return;

        CommitText();
        _textEditing = new VectorObject
        {
            Kind = VectorKind.Text,
            P1 = ToSkPhysical(p),
            Color = _brush.Color,
            FontSize = Math.Max(16f, _brush.Thickness * 5f),
        };
        // Pull keyboard focus off whatever toolbar button was last clicked so the
        // caret really receives keys; PreviewKeyDown covers the rest either way.
        Activate();
        Keyboard.Focus(this);
        RefreshDrawSurface();
    }

    /// <summary>Draws a caret at the end of the text being edited (feedback while typing).</summary>
    private void DrawTextCaret(SKCanvas canvas, VectorObject text)
    {
        using var measure = new SKPaint
        {
            TextSize = text.FontSize,
            Typeface = SKTypeface.FromFamilyName("Segoe UI"),
        };
        var lines = (text.Text ?? string.Empty).Split('\n');
        var last = lines.Length > 0 ? lines[^1] : string.Empty;
        var x = text.P1.X + measure.MeasureText(last);
        var yTop = text.P1.Y + (lines.Length - 1) * measure.FontSpacing;

        using var caret = new SKPaint
        {
            Color = text.Color,
            StrokeWidth = Math.Max(1.5f, text.FontSize / 16f),
            IsAntialias = true,
        };
        canvas.DrawLine(x + 1, yTop, x + 1, yTop + text.FontSize, caret);
    }

    private void CommitText()
    {
        if (_textEditing is null)
            return;

        if (!string.IsNullOrEmpty(_textEditing.Text) && _document is not null)
            CommitVectorToPaint(_textEditing);
        _textEditing = null;
        RefreshDrawSurface();
    }

    private void CancelText()
    {
        _textEditing = null;
        RefreshDrawSurface();
    }

    protected override void OnTextInput(TextCompositionEventArgs e)
    {
        base.OnTextInput(e);
        if (_textEditing is null || string.IsNullOrEmpty(e.Text))
            return;

        // Space is handled in OnKeyDown (a focused Window does not reliably raise
        // TextInput for it); skip it here so it is never inserted twice.
        foreach (var ch in e.Text)
            if (!char.IsControl(ch) && ch != ' ')
                _textEditing.Text += ch;
        RefreshDrawSurface();
        e.Handled = true;
    }

    // ---- Effects (blur / pixelize) ---------------------------------------------

    private void EnsureBaseBitmap()
    {
        if (_baseBitmap is not null)
            return;
        _baseBitmap = new SKBitmap(new SKImageInfo(_frame.Width, _frame.Height, SKColorType.Bgra8888, SKAlphaType.Opaque));
        Marshal.Copy(_frame.PixelsBgra, 0, _baseBitmap.GetPixels(), _frame.PixelsBgra.Length);
    }

    /// <summary>
    /// The effect source for the active tool. The Opacity slider drives strength:
    /// blur → sigma, pixelize → block size. The base is cached and only rebuilt
    /// when that strength changes.
    /// </summary>
    private SKBitmap EffectSource()
    {
        EnsureBaseBitmap();
        var info = new SKImageInfo(_frame.Width, _frame.Height, SKColorType.Bgra8888, SKAlphaType.Opaque);
        var strength = Math.Clamp(_brush.Opacity, 0f, 1f);

        if (_drawSub == DrawSubTool.Pixelize)
        {
            // Higher opacity → bigger blocks (coarser downscale factor).
            // 4× gentler than before: full slider now maps to the old ~25%.
            var factor = Math.Max(2, (int)Math.Round(3 + strength * 11.25));
            if (_pixelBase is null || _pixelFactor != factor)
            {
                _pixelBase?.Dispose();
                var sw = Math.Max(1, _frame.Width / factor);
                var sh = Math.Max(1, _frame.Height / factor);
                using var small = _baseBitmap!.Resize(new SKImageInfo(sw, sh, SKColorType.Bgra8888, SKAlphaType.Opaque), SKFilterQuality.Medium);
                _pixelBase = small.Resize(info, SKFilterQuality.None); // nearest-neighbour upscale
                _pixelFactor = factor;
            }
            return _pixelBase!;
        }

        // Higher opacity → stronger blur (bigger sigma).
        var sigma = Math.Max(1f, strength * 10.9375f);
        if (_blurBase is null || Math.Abs(_blurSigma - sigma) > 0.01f)
        {
            _blurBase?.Dispose();
            _blurBase = new SKBitmap(info);
            using var canvas = new SKCanvas(_blurBase);
            using var paint = new SKPaint { ImageFilter = SKImageFilter.CreateBlur(sigma, sigma) };
            canvas.DrawBitmap(_baseBitmap!, 0, 0, paint);
            _blurSigma = sigma;
        }
        return _blurBase!;
    }

    /// <summary>A repeating grey checkerboard shader (the "transparent" indicator).</summary>
    private SKShader CheckerShader()
    {
        if (_checkerShader is not null)
            return _checkerShader;

        const int cell = 8;
        using var tile = new SKBitmap(cell * 2, cell * 2);
        using (var c = new SKCanvas(tile))
        {
            c.Clear(new SKColor(0xC8, 0xC8, 0xC8));
            using var dark = new SKPaint { Color = new SKColor(0x80, 0x80, 0x80) };
            c.DrawRect(0, 0, cell, cell, dark);
            c.DrawRect(cell, cell, cell, cell, dark);
        }
        _checkerShader = SKShader.CreateBitmap(tile, SKShaderTileMode.Repeat, SKShaderTileMode.Repeat);
        return _checkerShader;
    }

    private void BeginEffect(Point p)
    {
        if (!AllSelections().Any())
            return;

        _effectStart = ToSkPhysical(p);
        _effectRectMode = Keyboard.Modifiers.HasFlag(ModifierKeys.Control);
        _effectRect = SKRect.Create(_effectStart.X, _effectStart.Y, 0, 0);
        _effectStroke = new SKPath();
        if (!_effectRectMode)
            _effectStroke.MoveTo(_effectStart);

        _strokeClip?.Dispose();
        _strokeClip = BuildSelectionClip();
        CaptureMouse();
        RefreshDrawSurface();
    }

    private void UpdateEffect(SKPoint cur)
    {
        if (_effectRectMode)
            _effectRect = SKRect.Create(
                Math.Min(_effectStart.X, cur.X), Math.Min(_effectStart.Y, cur.Y),
                Math.Abs(cur.X - _effectStart.X), Math.Abs(cur.Y - _effectStart.Y));
        else
            _effectStroke!.LineTo(cur);
    }

    /// <summary>The filled area affected by the current effect gesture (rect or brushed path).</summary>
    private SKPath EffectArea()
    {
        var area = new SKPath();
        if (_effectRectMode)
        {
            area.AddRect(_effectRect);
            return area;
        }

        using var stroke = new SKPaint
        {
            Style = SKPaintStyle.Stroke,
            StrokeWidth = Math.Max(10f, _brush.Thickness),
            StrokeCap = SKStrokeCap.Round,
            StrokeJoin = SKStrokeJoin.Round,
        };
        stroke.GetFillPath(_effectStroke!, area);
        return area;
    }

    /// <summary>
    /// Builds the soft-eraser coverage for the current gesture over <paramref name="region"/>:
    /// an opaque greyscale bitmap whose luminance is the erase strength. Brush mode stamps
    /// radial discs (centre = 100%, rim = Hardness) with a Lighten (max) blend so overlapping
    /// stamps along the stroke never compound past the intended profile. Rect mode (Ctrl-drag)
    /// fills flat at full strength.
    /// </summary>
    private SKBitmap BuildEraseCoverage(SKRectI region)
    {
        float hardness = Math.Clamp(_brush.Hardness, 0f, 1f);
        byte centre = 255;                            // centre always erases fully
        byte rim = (byte)Math.Round(hardness * 255);  // rim fades to the hardness value

        var cov = new SKBitmap(new SKImageInfo(region.Width, region.Height, SKColorType.Bgra8888, SKAlphaType.Opaque));
        using var canvas = new SKCanvas(cov);
        canvas.Clear(SKColors.Black);
        canvas.Translate(-region.Left, -region.Top); // stamp in document coordinates

        if (_effectRectMode)
        {
            using var fill = new SKPaint { Color = new SKColor(centre, centre, centre) };
            canvas.DrawRect(_effectRect, fill);
        }
        else if (_effectStroke is not null)
        {
            float radius = Math.Max(10f, _brush.Thickness) / 2f;
            var centreColor = new SKColor(centre, centre, centre);
            var rimColor = new SKColor(rim, rim, rim);
            using var paint = new SKPaint { IsAntialias = true, BlendMode = SKBlendMode.Lighten };
            foreach (var pt in FlattenStroke(_effectStroke, radius * 0.35f))
            {
                paint.Shader = SKShader.CreateRadialGradient(
                    pt, radius,
                    new[] { centreColor, rimColor },
                    new[] { 0f, 1f },
                    SKShaderTileMode.Clamp);
                canvas.DrawCircle(pt, radius, paint);
                paint.Shader.Dispose();
            }
        }
        return cov;
    }

    /// <summary>Samples a polyline path into points spaced ~<paramref name="spacing"/> apart.</summary>
    private static List<SKPoint> FlattenStroke(SKPath path, float spacing)
    {
        var verts = path.Points;
        var pts = new List<SKPoint>();
        if (verts.Length == 0)
            return pts;
        spacing = Math.Max(1f, spacing);
        pts.Add(verts[0]);
        for (int i = 1; i < verts.Length; i++)
        {
            var a = verts[i - 1];
            var b = verts[i];
            float dx = b.X - a.X, dy = b.Y - a.Y;
            float dist = (float)Math.Sqrt(dx * dx + dy * dy);
            int steps = Math.Max(1, (int)(dist / spacing));
            for (int s = 1; s <= steps; s++)
            {
                float t = s / (float)steps;
                pts.Add(new SKPoint(a.X + dx * t, a.Y + dy * t));
            }
        }
        return pts;
    }

    /// <summary>Soft-erases (DstOut) the current canvas layer by a coverage bitmap.</summary>
    private static void SoftEraseOnCanvas(SKCanvas canvas, SKBitmap coverage, SKRectI region, SKPath? clip)
    {
        canvas.Save();
        if (clip is not null)
            canvas.ClipPath(clip, antialias: true);
        using var filter = CaptureDocument.CreateCoverageAlphaFilter();
        using var paint = new SKPaint { BlendMode = SKBlendMode.DstOut, ColorFilter = filter };
        canvas.DrawBitmap(coverage, region.Left, region.Top, paint);
        canvas.Restore();
    }

    private void EndEffect()
    {
        ReleaseMouseCapture();
        if (_document is not null && _effectStroke is not null)
        {
            using var area = EffectArea();
            var region = RectToRegion(area.Bounds);
            if (region is { Width: > 0, Height: > 0 })
            {
                switch (_drawSub)
                {
                    case DrawSubTool.Blur or DrawSubTool.Pixelize:
                        var src = EffectSource();
                        ApplyRegionOp(_document.EffectsLayer, region, () => _document.ApplyEffect(src, area, _strokeClip));
                        break;
                    case DrawSubTool.FilterEraser:
                    {
                        using var cov = BuildEraseCoverage(region);
                        ApplyRegionOp(_document.EffectsLayer, region,
                            () => _document.EraseEffectsCoverage(cov, region.Left, region.Top, _strokeClip));
                        break;
                    }
                    case DrawSubTool.Eraser:
                    {
                        // Shapes/text are baked into the paint layer, so clearing paint
                        // pixels erases them by touch, no whole-object removal, and the
                        // effects layer is deliberately left untouched (that is the
                        // Filter Eraser's job). Global opacity + Hardness shape the erase.
                        using var cov = BuildEraseCoverage(region);
                        ApplyRegionOp(_document.PaintLayer, region,
                            () => _document.ErasePaintCoverage(cov, region.Left, region.Top, _strokeClip));
                        break;
                    }
                    case DrawSubTool.AbsoluteEraser:
                    {
                        using var cov = BuildEraseCoverage(region);
                        ApplyRegionOp(_document.AbsoluteMask, region,
                            () => _document.AbsoluteEraseCoverage(cov, region.Left, region.Top, _strokeClip));
                        _absoluteBounds = _absoluteBounds.IsEmpty
                            ? area.Bounds
                            : SKRect.Union(_absoluteBounds, area.Bounds);
                        break;
                    }
                }
            }
        }

        _effectStroke?.Dispose();
        _effectStroke = null;
        _strokeClip?.Dispose();
        _strokeClip = null;
        _effectRectMode = false;
        RefreshDrawSurface();
    }

    /// <summary>Snapshots a layer region, runs the op, snapshots again, and records undo.</summary>
    private void ApplyRegionOp(SKBitmap layer, SKRectI region, Action op)
    {
        var before = CaptureDocument.SnapshotRegion(layer, region);
        op();
        var after = CaptureDocument.SnapshotRegion(layer, region);
        _history.Push(new LayerRegionCommand(layer, region, before, after));
    }

    private SKRectI RectToRegion(SKRect b)
    {
        var x0 = Math.Clamp((int)Math.Floor(b.Left), 0, _frame.Width);
        var y0 = Math.Clamp((int)Math.Floor(b.Top), 0, _frame.Height);
        var x1 = Math.Clamp((int)Math.Ceiling(b.Right), 0, _frame.Width);
        var y1 = Math.Clamp((int)Math.Ceiling(b.Bottom), 0, _frame.Height);
        return x1 > x0 && y1 > y0 ? new SKRectI(x0, y0, x1, y1) : SKRectI.Empty;
    }

    private SKPoint ToSkPhysical(Point p) => new((float)(p.X * _dpiScaleX), (float)(p.Y * _dpiScaleY));

    private void BeginBrush(Point p)
    {
        // Drawing only works inside a selection (SPEC §3).
        if (!AllSelections().Any())
            return;

        _strokeClip?.Dispose();
        _strokeClip = BuildSelectionClip();
        _stroke = new BrushStroke(_brush);
        _stroke.Begin(ToSkPhysical(p));
        CaptureMouse();
        RefreshDrawSurface();
    }

    private void EndBrush()
    {
        if (_stroke is { IsEmpty: false } && _document is not null)
        {
            var region = StrokeRegion(_stroke.Path);
            if (region is { Width: > 0, Height: > 0 })
            {
                var before = CaptureDocument.SnapshotRegion(_document.PaintLayer, region);
                using (var paint = _stroke.CreatePaint())
                    _document.CommitStroke(_stroke.Path, paint, _strokeClip);
                var after = CaptureDocument.SnapshotRegion(_document.PaintLayer, region);
                _history.Push(new LayerRegionCommand(_document.PaintLayer, region, before, after));
            }
        }

        _stroke = null;
        _strokeClip?.Dispose();
        _strokeClip = null;
        ReleaseMouseCapture();
        RefreshDrawSurface();
    }

    /// <summary>The stroke's affected region (bounds + brush radius), clamped to the layer.</summary>
    private SKRectI StrokeRegion(SKPath path)
    {
        var b = path.Bounds;
        var pad = _brush.Thickness / 2f + 2f;
        var x0 = Math.Max(0, (int)Math.Floor(b.Left - pad));
        var y0 = Math.Max(0, (int)Math.Floor(b.Top - pad));
        var x1 = Math.Min(_frame.Width, (int)Math.Ceiling(b.Right + pad));
        var y1 = Math.Min(_frame.Height, (int)Math.Ceiling(b.Bottom + pad));
        return x1 > x0 && y1 > y0 ? new SKRectI(x0, y0, x1, y1) : SKRectI.Empty;
    }

    private void Undo()
    {
        if (_history.Undo())
            RefreshDrawSurface();
    }

    private void Redo()
    {
        if (_history.Redo())
            RefreshDrawSurface();
    }

    /// <summary>Combined selection outline as a physical-pixel SKPath (clip for drawing).</summary>
    private SKPath? BuildSelectionClip()
    {
        var path = new SKPath();
        foreach (var s in AllSelections())
        {
            using var sp = SkiaInterop.ToSkPath(s.BuildGeometry(), _dpiScaleX, _dpiScaleY);
            path.AddPath(sp);
        }
        return path.IsEmpty ? null : path;
    }

    // ---- OCR (text extraction) -------------------------------------------------

    /// <summary>Bounding box of the current selection(s) in frame pixels, or null.</summary>
    private SKRectI? OcrSelectionRegion()
    {
        var sels = AllSelections().ToList();
        if (sels.Count == 0)
            return null;
        var rects = sels.Select(s => BoundsToPixels(s.Bounds)).ToList();
        var u = ClampToFrame(UnionRects(rects));
        if (u.Width <= 0 || u.Height <= 0)
            return null;
        return new SKRectI(u.X, u.Y, u.X + u.Width, u.Y + u.Height);
    }

    /// <summary>Recognises text in the selection and enters selectable-text mode.</summary>
    private async void RunOcr()
    {
        if (_ocrBusy)
            return;

        if (!OcrService.IsAvailable)
        {
            ShowOcrHint("No OCR language pack installed (Settings ▸ Language ▸ add an OCR feature).");
            return;
        }

        var region = OcrSelectionRegion();
        if (region is not { } r)
        {
            ShowOcrHint("Select an area first, then press the OCR tool.");
            return;
        }

        _ocrBusy = true;
        _ocrMode = true;
        _ocr = null;
        _ocrSelecting = false;
        Cursor = Cursors.IBeam;
        HighlightTool();
        ShowOcrHint($"Recognising ({OcrModeLabel()})…");
        RefreshDrawSurface();

        try
        {
            var result = await OcrService.RecognizeAsync(_frame.PixelsBgra, _frame.Stride, r, _ocrLang);
            _ocr = new OcrTextLayer(result.Words);
            if (result.IsEmpty)
                ShowOcrHint($"No text found ({OcrModeLabel()}). Right-click the tool to switch mode.");
            else
                UpdateOcrHint();
        }
        catch (Exception ex)
        {
            Log.Error("OCR failed.", ex);
            _ocr = new OcrTextLayer(Array.Empty<OcrWord>());
            ShowOcrHint("OCR failed, see the log.");
        }
        finally
        {
            _ocrBusy = false;
            RefreshDrawSurface();
        }
    }

    private void ToggleOcrLanguage()
    {
        _ocrLang = _ocrLang switch { "auto" => "ru", "ru" => "en", _ => "auto" };
        if (_ocrMode)
            RunOcr(); // re-recognise the same selection in the new mode
        else
            ShowOcrHint($"OCR mode: {OcrModeLabel()}");
    }

    private string OcrModeLabel() => _ocrLang switch
    {
        "ru" => "RU",
        "en" => "EN",
        _ => "AUTO (RU+EN)",
    };

    private void CopyOcr()
    {
        if (_ocr is null)
            return;
        var text = _ocr.HasSelection ? _ocr.SelectedText() : _ocr.AllText();
        if (string.IsNullOrEmpty(text))
            return;
        try
        {
            System.Windows.Clipboard.SetText(text);
            var scope = _ocr.HasSelection ? "selection" : "all text";
            ShowOcrHint($"Copied {scope}, {text.Length} chars.");
        }
        catch (Exception ex)
        {
            Log.Error("OCR copy failed.", ex);
        }
    }

    private void ExitOcrMode()
    {
        if (!_ocrMode)
            return;
        _ocrMode = false;
        _ocrSelecting = false;
        _ocr = null;
        HideOcrHint();
        if (IsMouseCaptured)
            ReleaseMouseCapture();
        Cursor = Cursors.Cross;
        HighlightTool();
        RefreshDrawSurface();
    }

    private void OcrMouseDown(Point p)
    {
        if (_ocr is not { HasWords: true })
            return;
        _ocr.StartSelect(ToSkPhysical(p));
        _ocrSelecting = true;
        CaptureMouse();
        RefreshDrawSurface();
    }

    private void OcrMouseMove(Point p)
    {
        if (!_ocrSelecting || _ocr is null)
            return;
        _ocr.ExtendSelect(ToSkPhysical(p));
        RefreshDrawSurface();
    }

    private void OcrMouseUp()
    {
        if (!_ocrSelecting)
            return;
        _ocrSelecting = false;
        if (IsMouseCaptured)
            ReleaseMouseCapture();
        CopyOcr(); // selecting a span drops it straight onto the clipboard
        RefreshDrawSurface();
    }

    private void UpdateOcrHint() =>
        ShowOcrHint($"OCR {OcrModeLabel()} · drag to select · Ctrl+C copy · Ctrl+A all · right-click tool: mode · Esc exit");

    private void ShowOcrHint(string text)
    {
        OcrHintText.Text = text;
        OcrHint.Visibility = Visibility.Visible;
    }

    private void HideOcrHint() => OcrHint.Visibility = Visibility.Collapsed;

    private void RefreshDrawSurface() => DrawSurface.InvalidateVisual();

    private void OnPaintSurface(object? sender, SKPaintSurfaceEventArgs e)
    {
        var canvas = e.Surface.Canvas;
        canvas.Clear(SKColors.Transparent);
        if (_document is null)
        {
            // OCR highlights don't need a document (text may not have been drawn on).
            if (_ocrMode && _ocr is not null)
                _ocr.Render(canvas);
            return;
        }

        // While actively drawing, clip to the selection bounds: everything drawn is
        // inside the selection anyway, and on a large virtual desktop the full-frame
        // blits below otherwise cost the whole 4K surface every frame.
        if (_effectStroke is not null || _stroke is not null || _vectorPreview is not null)
        {
            var sel = AllSelections().Select(s => BoundsToPixels(s.Bounds)).ToList();
            if (sel.Count > 0)
            {
                var u = UnionRects(sel);
                canvas.ClipRect(SKRect.Create(u.X, u.Y, u.Width, u.Height));
            }
        }

        // Layers, bottom→top: effects → paint → vectors (matches export order).
        // Committed layers sit above the dim, so drawings stay visible while re-selecting.
        // A live Filter-Eraser softly removes effects inside an isolated layer so only the
        // effects fade (the base/dim below stay intact).
        if (_document.HasEffects)
        {
            if (_effectStroke is not null && _drawSub == DrawSubTool.FilterEraser)
            {
                using var area = EffectArea();
                var reg = RectToRegion(area.Bounds);
                canvas.SaveLayer(null);
                canvas.DrawBitmap(_document.EffectsLayer, 0, 0);
                if (reg is { Width: > 0, Height: > 0 })
                {
                    using var cov = BuildEraseCoverage(reg);
                    SoftEraseOnCanvas(canvas, cov, reg, _strokeClip);
                }
                canvas.Restore();
            }
            else
            {
                canvas.DrawBitmap(_document.EffectsLayer, 0, 0);
            }
        }

        // Blur/pixelize in-progress source preview (under paint).
        if (_effectStroke is not null && _drawSub is DrawSubTool.Blur or DrawSubTool.Pixelize)
        {
            using var area = EffectArea();
            canvas.Save();
            if (_strokeClip is not null)
                canvas.ClipPath(_strokeClip, antialias: true);
            canvas.ClipPath(area, antialias: true);
            canvas.DrawBitmap(EffectSource(), 0, 0);
            canvas.Restore();
        }

        // A live plain Eraser softly removes paint inside an isolated layer, revealing the
        // effects/base below (matching the committed result).
        if (_document.HasPaint)
        {
            if (_effectStroke is not null && _drawSub == DrawSubTool.Eraser)
            {
                using var area = EffectArea();
                var reg = RectToRegion(area.Bounds);
                canvas.SaveLayer(null);
                canvas.DrawBitmap(_document.PaintLayer, 0, 0);
                if (reg is { Width: > 0, Height: > 0 })
                {
                    using var cov = BuildEraseCoverage(reg);
                    SoftEraseOnCanvas(canvas, cov, reg, _strokeClip);
                }
                canvas.Restore();
            }
            else
            {
                canvas.DrawBitmap(_document.PaintLayer, 0, 0);
            }
        }

        // The in-progress stroke is clipped to the selection it began in.
        if (_stroke is { IsEmpty: false })
        {
            canvas.Save();
            if (_strokeClip is not null)
                canvas.ClipPath(_strokeClip, antialias: true);
            using var paint = _stroke.CreatePaint();
            canvas.DrawPath(_stroke.Path, paint);
            canvas.Restore();
        }

        // Committed shapes/text now live in the paint layer (drawn above); only the
        // in-progress preview and the text being typed are drawn as live vectors.
        _vectorPreview?.Draw(canvas);
        if (_textEditing is not null)
        {
            _textEditing.Draw(canvas);
            DrawTextCaret(canvas, _textEditing);
        }

        // Committed absolute-erased areas show a transparency checkerboard (the
        // pixels become transparent on export; this is the on-screen cue for it).
        if (_document.HasAbsolute && !_absoluteBounds.IsEmpty)
        {
            canvas.SaveLayer(_absoluteBounds, null);
            using (var checker = new SKPaint { Shader = CheckerShader() })
                canvas.DrawRect(_absoluteBounds, checker);
            using (var keep = new SKPaint { BlendMode = SKBlendMode.DstIn })
                canvas.DrawBitmap(_document.AbsoluteMask, 0, 0, keep);
            canvas.Restore();
        }

        // Absolute-eraser preview: a red tint whose strength follows the soft coverage
        // (these pixels become transparent on export). The plain Eraser and Filter Eraser
        // are previewed above by soft-erasing their own layers.
        if (_effectStroke is not null && _drawSub == DrawSubTool.AbsoluteEraser)
        {
            using var area = EffectArea();
            var reg = RectToRegion(area.Bounds);
            if (reg is { Width: > 0, Height: > 0 })
            {
                using var cov = BuildEraseCoverage(reg);
                var regRect = SKRect.Create(reg.Left, reg.Top, reg.Width, reg.Height);
                canvas.Save();
                if (_strokeClip is not null)
                    canvas.ClipPath(_strokeClip, antialias: true);
                canvas.SaveLayer(regRect, null);
                using (var red = new SKPaint { Color = new SKColor(0xFF, 0x2D, 0x55, 0xB0) })
                    canvas.DrawRect(regRect, red);
                using (var keep = new SKPaint
                {
                    BlendMode = SKBlendMode.DstIn,
                    ColorFilter = CaptureDocument.CreateCoverageAlphaFilter(),
                })
                    canvas.DrawBitmap(cov, reg.Left, reg.Top, keep);
                canvas.Restore();
                canvas.Restore();
            }
        }

        if (_brushCursorVisible && _tool == ToolMode.Draw && !_eyedropping && !_eyedropperArmed)
            DrawBrushCursor(canvas);

        // OCR selection highlights sit above every layer.
        if (_ocrMode && _ocr is not null)
            _ocr.Render(canvas);
    }

    /// <summary>Draws a ring the size of the brush at the cursor (double-stroked for contrast).</summary>
    private void DrawBrushCursor(SKCanvas canvas)
    {
        var r = Math.Max(2f, _brush.Thickness / 2f);
        using var outer = new SKPaint
        {
            Style = SKPaintStyle.Stroke, StrokeWidth = 2f,
            Color = new SKColor(0, 0, 0, 160), IsAntialias = true,
        };
        using var inner = new SKPaint
        {
            Style = SKPaintStyle.Stroke, StrokeWidth = 1f,
            Color = new SKColor(255, 255, 255, 230), IsAntialias = true,
        };
        canvas.DrawCircle(_cursorPhysical, r, outer);
        canvas.DrawCircle(_cursorPhysical, r, inner);
    }

    // ---- Brush settings panel (Shift) ------------------------------------------

    private static readonly string[] Palette =
    {
        "#FF3B30", "#FF9500", "#FFCC00", "#34C759", "#00C7BE", "#30B0C7", "#007AFF", "#5856D6",
        "#AF52DE", "#FF2D55", "#A2845E", "#8E8E93", "#FFFFFF", "#C7C7CC", "#48484A", "#000000",
    };

    private bool _syncingHex;

    private void WireBrushPanel()
    {
        ThicknessSlider.ValueChanged += (_, e) =>
        {
            _brush.Thickness = (float)e.NewValue;
            ThicknessValue.Text = $"{(int)Math.Round(e.NewValue)} px";
            RefreshDrawSurface(); // update the brush ring
        };
        OpacitySlider.ValueChanged += (_, e) =>
        {
            _brush.Opacity = (float)(e.NewValue / 100.0);
            OpacityValue.Text = $"{(int)Math.Round(e.NewValue)}%";
        };
        HardnessSlider.ValueChanged += (_, e) =>
        {
            _brush.Hardness = (float)(e.NewValue / 100.0);
            HardnessValue.Text = $"{(int)Math.Round(e.NewValue)}%";
        };

        // Click anywhere on the track jumps the thumb there AND keeps dragging in the
        // same gesture (no release + re-press). See EnableSliderClickDrag.
        EnableSliderClickDrag(ThicknessSlider);
        EnableSliderClickDrag(OpacitySlider);
        EnableSliderClickDrag(HardnessSlider);

        HexInput.TextChanged += (_, _) => TryApplyHex(HexInput.Text);

        SvSquare.MouseLeftButtonDown += (_, e) => { SvSquare.CaptureMouse(); UpdateSvFromMouse(e.GetPosition(SvSquare)); e.Handled = true; };
        SvSquare.MouseMove += (_, e) => { if (SvSquare.IsMouseCaptured) UpdateSvFromMouse(e.GetPosition(SvSquare)); };
        SvSquare.MouseLeftButtonUp += (_, e) => { SvSquare.ReleaseMouseCapture(); e.Handled = true; };
        HueBar.MouseLeftButtonDown += (_, e) => { HueBar.CaptureMouse(); UpdateHueFromMouse(e.GetPosition(HueBar)); e.Handled = true; };
        HueBar.MouseMove += (_, e) => { if (HueBar.IsMouseCaptured) UpdateHueFromMouse(e.GetPosition(HueBar)); };
        HueBar.MouseLeftButtonUp += (_, e) => { HueBar.ReleaseMouseCapture(); e.Handled = true; };
        ResetColorButton.Click += (_, _) => SetBrushColor(new SKColor(0xFF, 0x3B, 0x30));
        EyedropperButton.Click += (_, _) => ArmEyedropper(!_eyedropperArmed);

        // Sync the picker with the default brush color.
        DeriveHsv(_brush.Color);
        UpdatePickerUi();

        foreach (var hex in Palette)
        {
            var color = (Color)ColorConverter.ConvertFromString(hex);

            // Square, inset-bevelled well, the same swatch the settings app draws.
            var swatch = new Border
            {
                Background = new SolidColorBrush(color),
                Height = 16,
                Margin = new Thickness(1.5),
                Cursor = Cursors.Hand,
                BorderThickness = new Thickness(1, 1, 0, 0),
                BorderBrush = (Brush)FindResource("InputDark"),
                Child = new Border
                {
                    BorderThickness = new Thickness(0, 0, 1, 1),
                    BorderBrush = (Brush)FindResource("InputLight"),
                },
            };
            var captured = hex;
            swatch.MouseLeftButtonDown += (_, _) => HexInput.Text = captured;
            SwatchGrid.Children.Add(swatch);
        }
    }

    // Simple, predictable slider interaction: press anywhere on the track and the value
    // snaps to that point, then follows the cursor while held (no release needed), and
    // stops on release. Done entirely by hand, pressing captures the mouse on the slider
    // and drives Value straight from the cursor position, so there is no built-in Thumb
    // drag remembering a stale pre-jump origin.
    private void EnableSliderClickDrag(Slider slider)
    {
        slider.PreviewMouseLeftButtonDown += SliderClickDrag_Down;
        slider.PreviewMouseMove += SliderClickDrag_Move;
        slider.PreviewMouseLeftButtonUp += SliderClickDrag_Up;
    }

    private void SliderClickDrag_Down(object sender, MouseButtonEventArgs e)
    {
        var slider = (Slider)sender;
        SetSliderValueFromCursor(slider, e);
        slider.CaptureMouse();
        e.Handled = true;
    }

    private void SliderClickDrag_Move(object sender, MouseEventArgs e)
    {
        var slider = (Slider)sender;
        if (slider.IsMouseCaptured && e.LeftButton == MouseButtonState.Pressed)
            SetSliderValueFromCursor(slider, e);
    }

    private void SliderClickDrag_Up(object sender, MouseButtonEventArgs e)
    {
        var slider = (Slider)sender;
        if (!slider.IsMouseCaptured)
            return;
        slider.ReleaseMouseCapture();
        e.Handled = true;
    }

    private static void SetSliderValueFromCursor(Slider slider, MouseEventArgs e)
    {
        var track = FindTrack(slider);
        if (track == null)
            return;
        // Map the cursor's X straight to an absolute value: the thumb centre follows the
        // cursor, the ends of the usable track reach Minimum/Maximum. No dependency on the
        // thumb's current position, so a single click lands exactly where you press.
        double thumbWidth = track.Thumb?.ActualWidth ?? 0;
        double usable = track.ActualWidth - thumbWidth;
        if (usable <= 0)
            return;
        double x = e.GetPosition(track).X - thumbWidth / 2;
        double fraction = Math.Clamp(x / usable, 0, 1);
        if (track.IsDirectionReversed)
            fraction = 1 - fraction;
        slider.Value = slider.Minimum + fraction * (slider.Maximum - slider.Minimum);
    }

    private static System.Windows.Controls.Primitives.Track? FindTrack(DependencyObject root)
    {
        if (root is System.Windows.Controls.Primitives.Track track)
            return track;
        int count = VisualTreeHelper.GetChildrenCount(root);
        for (int i = 0; i < count; i++)
        {
            var found = FindTrack(VisualTreeHelper.GetChild(root, i));
            if (found != null)
                return found;
        }
        return null;
    }

    private void ToggleBrushPanel()
    {
        if (_brushPanelOpen)
            CloseBrushPanel();
        else
            OpenBrushPanel();
    }

    /// <summary>
    /// Slides the settings panel out from under the strip: the strip travels up by
    /// the panel's height while the panel grows into the gap it leaves behind, so the
    /// handle visibly drags the panel open. Shift and the strip share this path.
    /// </summary>
    private void OpenBrushPanel()
    {
        // Mid-flight toggles are ignored rather than stacked; hammering Shift used
        // to leave the strip and the panel animating against each other.
        if (_brushPanelOpen || _panelAnimating || SettingsStrip.Visibility != Visibility.Visible)
            return;

        _brushPanelOpen = true;
        _panelAnimating = true;
        BrushPanel.Visibility = Visibility.Visible;

        var (x, stripClosed, stripOpen, height) = ChromeGeometry();
        Canvas.SetLeft(BrushPanel, x);

        // Seed the closed pose before animating. Untouched Canvas.Top / Height read back as
        // NaN, which Animate treats as 0, on the very first open of each tool that made the
        // panel fly in from the top of the screen instead of unfolding from under the strip.
        if (double.IsNaN(Canvas.GetTop(BrushPanel)))
            Canvas.SetTop(BrushPanel, stripClosed + SettingsStrip.Height);
        if (double.IsNaN(BrushPanel.Height))
            BrushPanel.Height = 0;
        if (double.IsNaN(Canvas.GetTop(SettingsStrip)))
            Canvas.SetTop(SettingsStrip, stripClosed);

        Animate(SettingsStrip, Canvas.TopProperty, stripOpen);
        Animate(BrushPanel, Canvas.TopProperty, stripOpen + SettingsStrip.Height);
        Animate(BrushPanel, HeightProperty, height, () => _panelAnimating = false);
    }

    private void CloseBrushPanel()
    {
        if (!_brushPanelOpen)
        {
            if (!_panelAnimating)
                BrushPanel.Visibility = Visibility.Collapsed;
            return;
        }

        if (_panelAnimating)
            return;

        _brushPanelOpen = false;
        _panelAnimating = true;

        var (_, stripClosed, _, _) = ChromeGeometry();

        Animate(SettingsStrip, Canvas.TopProperty, stripClosed);
        Animate(BrushPanel, Canvas.TopProperty, stripClosed + SettingsStrip.Height);
        Animate(BrushPanel, HeightProperty, 0, () =>
        {
            _panelAnimating = false;
            // A reopen may have overtaken this animation.
            if (!_brushPanelOpen)
                BrushPanel.Visibility = Visibility.Collapsed;
        });
    }

    /// <summary>
    /// Where the strip sits closed and open, and how tall the panel wants to be.
    /// The panel's bottom always rests just above the toolbar.
    /// </summary>
    private (double X, double StripClosed, double StripOpen, double Height) ChromeGeometry()
    {
        const double gap = 3;

        var toolbarX = Canvas.GetLeft(Toolbar);
        var toolbarY = Canvas.GetTop(Toolbar);
        if (double.IsNaN(toolbarX)) toolbarX = 24;
        if (double.IsNaN(toolbarY)) toolbarY = 120;

        var height = MeasurePanelHeight();
        var x = Math.Clamp(toolbarX, 0, Math.Max(0, RootGrid.ActualWidth - BrushPanel.Width));

        var stripClosed = toolbarY - SettingsStrip.Height - gap;
        var stripOpen = toolbarY - gap - height - SettingsStrip.Height;

        // Too tall to fit above the toolbar: pin it to the top edge instead.
        if (stripOpen < 0)
            stripOpen = 0;

        return (x, stripClosed, stripOpen, height);
    }

    /// <summary>Natural height of the panel for the active tool (sections vary).</summary>
    private double MeasurePanelHeight()
    {
        BrushPanel.BeginAnimation(HeightProperty, null);
        BrushPanel.Height = double.NaN;
        BrushPanel.Measure(new Size(BrushPanel.Width, double.PositiveInfinity));
        return BrushPanel.DesiredSize.Height;
    }

    private static readonly Duration ChromeDuration = new(TimeSpan.FromSeconds(0.3));

    private static readonly IEasingFunction ChromeEase =
        new CubicEase { EasingMode = EasingMode.EaseOut };

    /// <summary>
    /// Animates a property and then hands the value back to the property system, so
    /// later direct writes (a moving selection, say) are not blocked by a held clock.
    /// </summary>
    private static void Animate(
        DependencyObject target, DependencyProperty property, double to, Action? onDone = null)
    {
        // Both UIElement and Transform animate through IAnimatable. Starting from the
        // property's current value (the animated one, mid-flight) keeps an interrupted
        // run continuous instead of snapping back to a nominal start.
        var animatable = (IAnimatable)target;
        var current = target.GetValue(property);
        var from = current is double d && !double.IsNaN(d) ? d : 0d;

        var anim = new DoubleAnimation(from, to, ChromeDuration)
        {
            EasingFunction = ChromeEase,
            FillBehavior = FillBehavior.HoldEnd,
        };
        anim.Completed += (_, _) =>
        {
            animatable.BeginAnimation(property, null);
            target.SetValue(property, to);
            onDone?.Invoke();
        };
        animatable.BeginAnimation(property, anim);
    }

    private void PositionBrushPanel()
    {
        if (!_brushPanelOpen)
            return;

        // Re-anchor without animating: the selection moved under an open panel.
        var (x, _, stripOpen, height) = ChromeGeometry();

        SettingsStrip.BeginAnimation(Canvas.TopProperty, null);
        BrushPanel.BeginAnimation(Canvas.TopProperty, null);
        BrushPanel.BeginAnimation(HeightProperty, null);

        Canvas.SetTop(SettingsStrip, stripOpen);
        Canvas.SetLeft(BrushPanel, x);
        Canvas.SetTop(BrushPanel, stripOpen + SettingsStrip.Height);
        BrushPanel.Height = height;
    }

    private void TryApplyHex(string text)
    {
        if (_syncingHex)
            return;
        try
        {
            var c = (Color)ColorConverter.ConvertFromString(text.Trim());
            var sk = new SKColor(c.R, c.G, c.B);
            _brush.Color = sk;
            ColorPreview.Background = new SolidColorBrush(Color.FromRgb(c.R, c.G, c.B));
            DeriveHsv(sk);
            UpdatePickerUi();
        }
        catch
        {
            // Partial/invalid hex while typing, ignore until it parses.
        }
    }

    private void SetBrushColor(SKColor color)
    {
        var rgb = new SKColor(color.Red, color.Green, color.Blue);
        _brush.Color = rgb;
        _syncingHex = true;
        HexInput.Text = $"#{color.Red:X2}{color.Green:X2}{color.Blue:X2}";
        _syncingHex = false;
        ColorPreview.Background = new SolidColorBrush(Color.FromRgb(color.Red, color.Green, color.Blue));
        DeriveHsv(rgb);
        UpdatePickerUi();
    }

    private void UpdateSvFromMouse(Point p)
    {
        _sat = Math.Clamp(p.X / SvW, 0, 1);
        _val = Math.Clamp(1 - p.Y / SvH, 0, 1);
        ApplyHsv();
    }

    private void UpdateHueFromMouse(Point p)
    {
        _hue = Math.Clamp(p.Y / HueH, 0, 1) * 360;
        UpdateSvHueFill();
        ApplyHsv();
    }

    /// <summary>Turns the current H/S/V into the brush color and syncs hex + preview.</summary>
    private void ApplyHsv()
    {
        var color = SKColor.FromHsv((float)_hue, (float)(_sat * 100), (float)(_val * 100));
        _brush.Color = color;
        _syncingHex = true;
        HexInput.Text = $"#{color.Red:X2}{color.Green:X2}{color.Blue:X2}";
        _syncingHex = false;
        ColorPreview.Background = new SolidColorBrush(Color.FromRgb(color.Red, color.Green, color.Blue));
        UpdateThumbs();
    }

    private void DeriveHsv(SKColor color)
    {
        color.ToHsv(out var h, out var s, out var v);
        _hue = h;
        _sat = s / 100.0;
        _val = v / 100.0;
    }

    private void UpdatePickerUi()
    {
        UpdateSvHueFill();
        UpdateThumbs();
    }

    private void UpdateSvHueFill()
    {
        var c = SKColor.FromHsv((float)_hue, 100, 100);
        SvHueFill.Fill = new SolidColorBrush(Color.FromRgb(c.Red, c.Green, c.Blue));
    }

    private void UpdateThumbs()
    {
        Canvas.SetLeft(SvThumb, _sat * SvW - 6);
        Canvas.SetTop(SvThumb, (1 - _val) * SvH - 6);
        Canvas.SetTop(HueThumb, _hue / 360.0 * HueH - 2);
    }

    // ---- Eyedropper (RMB-hold in Draw mode) ------------------------------------

    private void UpdateEyedropper(Point p)
    {
        var color = SampleColor(p);
        EyedropperSwatch.Background = new SolidColorBrush(Color.FromRgb(color.Red, color.Green, color.Blue));
        EyedropperHex.Text = $"#{color.Red:X2}{color.Green:X2}{color.Blue:X2}";
        EyedropperPreview.Visibility = Visibility.Visible;
        EyedropperPreview.UpdateLayout();

        var x = Math.Clamp(p.X + 18, 0, Math.Max(0, RootGrid.ActualWidth - EyedropperPreview.ActualWidth));
        var y = Math.Clamp(p.Y - EyedropperPreview.ActualHeight - 12, 0, RootGrid.ActualHeight);
        Canvas.SetLeft(EyedropperPreview, x);
        Canvas.SetTop(EyedropperPreview, y);
    }

    private void CommitEyedropper(Point p)
    {
        SetBrushColor(SampleColor(p));
        Log.Info("Eyedropper: color picked.");
    }

    /// <summary>
    /// Arms/disarms the panel eyedropper. While armed the preview follows the cursor with no
    /// button held, and the next right-click takes the colour (see OnMouseRightButtonDown).
    /// </summary>
    private void ArmEyedropper(bool armed)
    {
        _eyedropperArmed = armed;
        EyedropperButton.Tag = armed ? "on" : null;
        Cursor = armed ? Cursors.Cross : (_drawSub == DrawSubTool.Text ? Cursors.IBeam : Cursors.Cross);

        if (!armed)
            EyedropperPreview.Visibility = Visibility.Collapsed;
        RefreshDrawSurface(); // the brush ring hides while picking
        Log.Info($"Eyedropper: {(armed ? "armed (right-click to pick)" : "disarmed")}.");
    }

    /// <summary>Samples the composited color (frame + paint) under a DIP point.</summary>
    private SKColor SampleColor(Point p)
    {
        var px = Math.Clamp((int)Math.Round(p.X * _dpiScaleX), 0, _frame.Width - 1);
        var py = Math.Clamp((int)Math.Round(p.Y * _dpiScaleY), 0, _frame.Height - 1);
        var i = (py * _frame.Width + px) * 4;
        byte b = _frame.PixelsBgra[i], g = _frame.PixelsBgra[i + 1], r = _frame.PixelsBgra[i + 2];

        if (_document is { HasPaint: true })
        {
            var pc = _document.PaintLayer.GetPixel(px, py);
            if (pc.Alpha > 0)
            {
                var a = pc.Alpha / 255f;
                byte Mix(byte s, byte d) => (byte)Math.Round(s * a + d * (1 - a));
                return new SKColor(Mix(pc.Red, r), Mix(pc.Green, g), Mix(pc.Blue, b));
            }
        }
        return new SKColor(r, g, b);
    }

    private void PrepGesture(Point p)
    {
        _dragStart = p;
        _shiftAtStart = Keyboard.Modifiers.HasFlag(ModifierKeys.Shift);
        _lockedRatio = null;
        _ctrlAMode = CtrlAMode.None;
    }

    private void BeginDragSelection(Point p)
    {
        _dragMode = DragMode.NewSelection;
        _freehandPoints.Clear();

        if (_activeShape == ShapeKind.Lasso)
        {
            _freehandPoints.Add(p);
            _selection = new Selection { Kind = ShapeKind.Lasso, Bounds = new Rect(p, new Size(0, 0)) };
        }
        else
        {
            _selection = new Selection { Kind = _activeShape, Bounds = new Rect(p, new Size(0, 0)) };
        }
    }

    // ---- Polygon click-mode ----------------------------------------------------

    private void StartPolygon(Point p)
    {
        _selection = null; // a new polygon replaces any current selection
        _polygonInProgress = true;
        _polygonPoints.Clear();
        _polygonPoints.Add(p);
        _polygonCursor = p;
        UpdateVisuals();
    }

    private void HandlePolygonClick(Point p, int clickCount)
    {
        if (clickCount == 2)
        {
            ClosePolygon();
            return;
        }

        // Clicking back on the first vertex closes the outline.
        if (_polygonPoints.Count >= 3 && (p - _polygonPoints[0]).Length <= PolygonCloseThreshold)
        {
            ClosePolygon();
            return;
        }

        _polygonPoints.Add(p);
        _polygonCursor = p;
        UpdateVisuals();
    }

    private void ClosePolygon()
    {
        if (_polygonPoints.Count >= 3)
        {
            var bounds = Selection.BoundsOf(_polygonPoints);
            _selection = new Selection
            {
                Kind = ShapeKind.Polygon,
                Bounds = bounds,
                NormalizedPoints = Selection.Normalize(_polygonPoints, bounds),
            };
        }

        _polygonInProgress = false;
        _polygonPoints.Clear();
        UpdateVisuals();
    }

    private void CancelPolygon()
    {
        _polygonInProgress = false;
        _polygonPoints.Clear();
        UpdateVisuals();
    }

    protected override void OnMouseMove(MouseEventArgs e)
    {
        base.OnMouseMove(e);
        var p = e.GetPosition(RootGrid);

        // OCR mode: drag extends the text selection.
        if (_ocrMode)
        {
            OcrMouseMove(p);
            return;
        }

        // Eyedropper: follow the cursor, previewing the color underneath. The armed mode
        // does this with no button held; over HUD chrome the preview steps aside.
        if (_eyedropping)
        {
            UpdateEyedropper(p);
            return;
        }
        if (_eyedropperArmed)
        {
            if (IsOverChrome())
                EyedropperPreview.Visibility = Visibility.Collapsed;
            else
                UpdateEyedropper(p);
            return;
        }

        // Polygon in progress: rubber-band a line from the last vertex to the cursor.
        if (_polygonInProgress)
        {
            _polygonCursor = p;
            UpdateVisuals();
            return;
        }

        // Draw tool: track the cursor (brush ring), extend a stroke or shape.
        if (_tool == ToolMode.Draw)
        {
            _cursorPhysical = ToSkPhysical(p);
            _brushCursorVisible = _drawSub == DrawSubTool.Brush || IsRegionTool(_drawSub);
            _stroke?.Extend(_cursorPhysical);
            if (_vectorPreview is not null)
                UpdateShape(_cursorPhysical);
            if (_effectStroke is not null)
                UpdateEffect(_cursorPhysical);
            RefreshDrawSurface();
            return;
        }

        if (_dragMode == DragMode.None)
        {
            UpdateHoverCursor(p);
            return;
        }

        var shiftNow = Keyboard.Modifiers.HasFlag(ModifierKeys.Shift);

        switch (_dragMode)
        {
            case DragMode.NewSelection when _activeShape == ShapeKind.Lasso:
                _freehandPoints.Add(p);
                UpdateFreehandSelection();
                break;

            case DragMode.NewSelection:
                SetBounds(ClampRect(RectFromAnchor(_dragStart, p, RatioFor(shiftNow))));
                break;

            case DragMode.Move:
                SetBounds(ClampMove(_refRect, p - _dragStart));
                break;

            case DragMode.Resize:
                SetBounds(ClampRect(ResizeRect(_activeHandle, _refRect, p, shiftNow)));
                break;
        }

        UpdateVisuals();
    }

    protected override void OnMouseLeftButtonUp(MouseButtonEventArgs e)
    {
        base.OnMouseLeftButtonUp(e);

        if (_ocrMode)
        {
            OcrMouseUp();
            return;
        }

        if (_tool == ToolMode.Draw && _stroke is not null)
        {
            EndBrush();
            return;
        }

        if (_tool == ToolMode.Draw && _vectorPreview is not null)
        {
            EndShape();
            return;
        }

        if (_tool == ToolMode.Draw && _effectStroke is not null)
        {
            EndEffect();
            return;
        }

        if (_dragMode == DragMode.None)
            return;

        var wasNew = _dragMode == DragMode.NewSelection;
        _dragMode = DragMode.None;
        _activeHandle = Handle.None;
        ReleaseMouseCapture();

        // A stray click (no real drag) while starting a new selection clears it.
        if (wasNew && _selection is not { Bounds.Width: >= 3, Bounds.Height: >= 3 })
            _selection = null;

        UpdateVisuals();
    }

    protected override void OnMouseRightButtonDown(MouseButtonEventArgs e)
    {
        base.OnMouseRightButtonDown(e);

        // Right-click auto-closes an in-progress polygon (straight last→first, SPEC §4.2).
        if (_polygonInProgress)
        {
            ClosePolygon();
            e.Handled = true;
            return;
        }

        // Armed from the panel button: this right-click is the pick, and it ends the mode.
        if (_eyedropperArmed && !IsOverChrome())
        {
            CommitEyedropper(e.GetPosition(RootGrid));
            ArmEyedropper(false);
            e.Handled = true;
            return;
        }

        // Draw mode: RMB-hold picks a color (eyedropper). HUD chrome keeps priority -
        // a right-click on a tab must open its flyout, not start the eyedropper.
        if (_tool == ToolMode.Draw && !IsOverChrome())
        {
            _eyedropping = true;
            CaptureMouse();
            UpdateEyedropper(e.GetPosition(RootGrid));
            e.Handled = true;
        }
    }

    /// <summary>True when the pointer is over any HUD panel rather than the canvas.</summary>
    private bool IsOverChrome() =>
        Toolbar.IsMouseOver || BrushPanel.IsMouseOver || SettingsStrip.IsMouseOver ||
        AllFlyouts().Any(f => f.IsMouseOver);

    protected override void OnMouseLeave(MouseEventArgs e)
    {
        base.OnMouseLeave(e);
        if (_brushCursorVisible)
        {
            _brushCursorVisible = false;
            RefreshDrawSurface();
        }
    }

    protected override void OnMouseRightButtonUp(MouseButtonEventArgs e)
    {
        base.OnMouseRightButtonUp(e);
        if (!_eyedropping)
            return;

        _eyedropping = false;
        ReleaseMouseCapture();
        CommitEyedropper(e.GetPosition(RootGrid));
        EyedropperPreview.Visibility = Visibility.Collapsed;
        e.Handled = true;
    }

    /// <summary>Rebuilds the lasso selection from the freehand points captured so far.</summary>
    private void UpdateFreehandSelection()
    {
        if (_freehandPoints.Count < 2)
            return;

        var bounds = Selection.BoundsOf(_freehandPoints);
        _selection = new Selection
        {
            Kind = ShapeKind.Lasso,
            Bounds = bounds,
            NormalizedPoints = Selection.Normalize(_freehandPoints, bounds),
        };
    }

    private void SetBounds(Rect bounds)
    {
        if (_selection is not null)
            _selection.Bounds = bounds;
    }

    // ---- Keyboard --------------------------------------------------------------

    /// <summary>
    /// Text-editing keys are intercepted here, not in OnKeyDown. PreviewKeyDown
    /// tunnels from the window down to whatever holds keyboard focus, so it runs
    /// before a focused toolbar Button can consume Space/Enter as a click, which
    /// is exactly why the earlier OnKeyDown handler never saw them (the Button ate
    /// the KeyDown and it never bubbled up to the window). Enter and Shift+Enter
    /// both insert a newline; the character keys still arrive via OnTextInput.
    /// </summary>
    protected override void OnPreviewKeyDown(KeyEventArgs e)
    {
        base.OnPreviewKeyDown(e);

        if (_textEditing is null)
            return;

        switch (e.Key)
        {
            case Key.Escape:
                // Finish editing but keep what was typed (losing text on Esc is a
                // footgun); a second Esc, with no text active, closes the overlay.
                CommitText();
                e.Handled = true;
                break;
            case Key.Enter:
                _textEditing.Text += '\n'; // commit is Esc / click-away / tool switch
                RefreshDrawSurface();
                e.Handled = true;
                break;
            case Key.Space:
                _textEditing.Text += ' ';
                RefreshDrawSurface();
                e.Handled = true;
                break;
            case Key.Back:
                if (_textEditing.Text.Length > 0)
                {
                    _textEditing.Text = _textEditing.Text[..^1];
                    RefreshDrawSurface();
                }
                e.Handled = true;
                break;
        }
    }

    protected override void OnKeyDown(KeyEventArgs e)
    {
        base.OnKeyDown(e);

        // While typing, editing keys are already handled in OnPreviewKeyDown; here we
        // only swallow the rest so tool shortcuts (1/2, Ctrl+Z…) don't fire mid-word.
        // No Handled is set for printable keys, so they still reach OnTextInput.
        if (_textEditing is not null)
            return;

        // While typing in the hex box, swallow shortcuts, but Escape still closes
        // the palette (and returns focus to the overlay).
        if (Keyboard.FocusedElement is TextBox)
        {
            if (e.Key == Key.Escape)
            {
                CloseBrushPanel();
                Keyboard.Focus(this);
                e.Handled = true;
            }
            return;
        }

        // OCR mode owns copy / select-all / exit; other keys fall through (a tool digit
        // then switches out of OCR via SetTool / EnterDrawMode).
        if (_ocrMode)
        {
            switch (e.Key)
            {
                case Key.C when Keyboard.Modifiers.HasFlag(ModifierKeys.Control):
                    CopyOcr();
                    e.Handled = true;
                    return;
                case Key.A when Keyboard.Modifiers.HasFlag(ModifierKeys.Control):
                    _ocr?.SelectAll();
                    CopyOcr();
                    RefreshDrawSurface();
                    e.Handled = true;
                    return;
                case Key.Escape:
                    ExitOcrMode();
                    e.Handled = true;
                    return;
            }
        }

        // Digits pick a toolbar slot, left to right (SPEC §12, extended to the full bar).
        // Checked before the switch so 1–9/0 stay one consistent family of shortcuts.
        if (!Keyboard.Modifiers.HasFlag(ModifierKeys.Control) && ToolSlot(e.Key) is { } slot)
        {
            ActivateToolSlot(slot);
            e.Handled = true;
            return;
        }

        switch (e.Key)
        {
            case Key.LeftShift or Key.RightShift:
                // Shift toggles the brush panel while drawing (SPEC §6.5), but not when
                // it's acting as a modifier (e.g. Ctrl+Shift+Z).
                if (_tool == ToolMode.Draw && !e.IsRepeat &&
                    !Keyboard.Modifiers.HasFlag(ModifierKeys.Control))
                    ToggleBrushPanel();
                break;

            case Key.A when Keyboard.Modifiers.HasFlag(ModifierKeys.Control):
                ApplyCtrlA();
                e.Handled = true;
                break;

            case Key.C when Keyboard.Modifiers.HasFlag(ModifierKeys.Control):
                // Ctrl+C = Copy + close, like Enter.
                if (_selection is { Bounds.Width: > 0, Bounds.Height: > 0 } || _committed.Count > 0)
                {
                    ExportAndClose(ExportKind.Copy);
                    e.Handled = true;
                }
                break;

            case Key.Z when Keyboard.Modifiers.HasFlag(ModifierKeys.Control) &&
                           Keyboard.Modifiers.HasFlag(ModifierKeys.Shift):
                Redo();
                e.Handled = true;
                break;

            case Key.Z when Keyboard.Modifiers.HasFlag(ModifierKeys.Control):
                Undo();
                e.Handled = true;
                break;

            case Key.Y when Keyboard.Modifiers.HasFlag(ModifierKeys.Control):
                Redo();
                e.Handled = true;
                break;


            case Key.Escape:
                // Esc backs out of the armed eyedropper first, then the palette, etc.
                if (_eyedropperArmed)
                    ArmEyedropper(false);
                else if (_brushPanelOpen)
                    CloseBrushPanel();
                else if (_polygonInProgress)
                    CancelPolygon();
                else
                    EndSession(produced: false);
                e.Handled = true;
                break;

            case Key.Enter:
                if (_polygonInProgress)
                {
                    ClosePolygon();
                    e.Handled = true;
                }
                else if (_selection is { Bounds.Width: > 0, Bounds.Height: > 0 })
                {
                    ExportAndClose(ExportKind.Copy);
                    e.Handled = true;
                }
                break;
        }
    }

    /// <summary>Ctrl+A selects the primary monitor; pressing it again expands to all.</summary>
    private void ApplyCtrlA()
    {
        _ctrlAMode = _ctrlAMode == CtrlAMode.Primary ? CtrlAMode.All : CtrlAMode.Primary;
        var bounds = _ctrlAMode == CtrlAMode.All ? AllMonitorsRect() : PrimaryMonitorRect();
        _committed.Clear();
        _selection = new Selection { Kind = ShapeKind.Rectangle, Bounds = bounds };
        _dragMode = DragMode.None;
        Log.Info($"Ctrl+A → {_ctrlAMode} monitor selection.");
        UpdateVisuals();
    }

    private Rect PrimaryMonitorRect()
    {
        var m = _frame.Monitors.FirstOrDefault(x => x.IsPrimary) ?? _frame.Monitors[0];
        var x = (m.Left - _frame.VirtualLeft) / _dpiScaleX;
        var y = (m.Top - _frame.VirtualTop) / _dpiScaleY;
        return new Rect(x, y, m.Width / _dpiScaleX, m.Height / _dpiScaleY);
    }

    private Rect AllMonitorsRect() => new(0, 0, RootGrid.ActualWidth, RootGrid.ActualHeight);

    // ---- Geometry helpers ------------------------------------------------------

    private double? RatioFor(bool shiftNow)
    {
        if (_shiftAtStart)
            return 1.0;

        if (shiftNow)
        {
            if (_lockedRatio is null && _selection is { Bounds: { Width: > 1, Height: > 1 } b })
                _lockedRatio = b.Width / b.Height;
            return _lockedRatio;
        }

        _lockedRatio = null;
        return null;
    }

    private static Rect RectFromAnchor(Point anchor, Point corner, double? ratio)
    {
        double dx = corner.X - anchor.X;
        double dy = corner.Y - anchor.Y;

        if (ratio is { } r && r > 0)
        {
            var aw = Math.Abs(dx);
            var ah = Math.Abs(dy);
            if (aw >= ah * r)
                ah = aw / r;
            else
                aw = ah * r;
            dx = aw * (dx < 0 ? -1 : 1);
            dy = ah * (dy < 0 ? -1 : 1);
        }

        var x = Math.Min(anchor.X, anchor.X + dx);
        var y = Math.Min(anchor.Y, anchor.Y + dy);
        return new Rect(x, y, Math.Abs(dx), Math.Abs(dy));
    }

    private Rect ResizeRect(Handle handle, Rect refRect, Point p, bool shiftNow)
    {
        switch (handle)
        {
            case Handle.TopLeft:
                return RectFromAnchor(refRect.BottomRight, p, RatioFor(shiftNow));
            case Handle.TopRight:
                return RectFromAnchor(new Point(refRect.Left, refRect.Bottom), p, RatioFor(shiftNow));
            case Handle.BottomLeft:
                return RectFromAnchor(new Point(refRect.Right, refRect.Top), p, RatioFor(shiftNow));
            case Handle.BottomRight:
                return RectFromAnchor(refRect.TopLeft, p, RatioFor(shiftNow));
        }

        double l = refRect.Left, t = refRect.Top, right = refRect.Right, b = refRect.Bottom;
        switch (handle)
        {
            case Handle.Top: t = p.Y; break;
            case Handle.Bottom: b = p.Y; break;
            case Handle.Left: l = p.X; break;
            case Handle.Right: right = p.X; break;
        }
        return new Rect(Math.Min(l, right), Math.Min(t, b), Math.Abs(right - l), Math.Abs(b - t));
    }

    private Rect ClampMove(Rect rect, Vector delta)
    {
        var x = Math.Clamp(rect.X + delta.X, 0, Math.Max(0, RootGrid.ActualWidth - rect.Width));
        var y = Math.Clamp(rect.Y + delta.Y, 0, Math.Max(0, RootGrid.ActualHeight - rect.Height));
        return new Rect(x, y, rect.Width, rect.Height);
    }

    private Rect ClampRect(Rect r)
    {
        var left = Math.Clamp(r.Left, 0, RootGrid.ActualWidth);
        var top = Math.Clamp(r.Top, 0, RootGrid.ActualHeight);
        var right = Math.Clamp(r.Right, 0, RootGrid.ActualWidth);
        var bottom = Math.Clamp(r.Bottom, 0, RootGrid.ActualHeight);
        return new Rect(left, top, Math.Max(0, right - left), Math.Max(0, bottom - top));
    }

    private static Rect Inflate(Rect r, double by) =>
        new(r.X - by, r.Y - by, r.Width + 2 * by, r.Height + 2 * by);

    private Handle HitTestHandle(Point p, Rect sel)
    {
        var tolerance = HandleSize / 2 + 3;
        foreach (Handle h in Enum.GetValues<Handle>())
        {
            if (h == Handle.None)
                continue;
            var c = HandleCenter(h, sel);
            if (Math.Abs(p.X - c.X) <= tolerance && Math.Abs(p.Y - c.Y) <= tolerance)
                return h;
        }
        return Handle.None;
    }

    private static Point HandleCenter(Handle h, Rect r) => h switch
    {
        Handle.TopLeft => new Point(r.Left, r.Top),
        Handle.Top => new Point(r.Left + r.Width / 2, r.Top),
        Handle.TopRight => new Point(r.Right, r.Top),
        Handle.Right => new Point(r.Right, r.Top + r.Height / 2),
        Handle.BottomRight => new Point(r.Right, r.Bottom),
        Handle.Bottom => new Point(r.Left + r.Width / 2, r.Bottom),
        Handle.BottomLeft => new Point(r.Left, r.Bottom),
        Handle.Left => new Point(r.Left, r.Top + r.Height / 2),
        _ => default,
    };

    private void UpdateHoverCursor(Point p)
    {
        if (_selection is { Bounds.Width: > 0, Bounds.Height: > 0 } sel)
        {
            var handle = HitTestHandle(p, sel.Bounds);
            if (handle != Handle.None)
            {
                Cursor = CursorForHandle(handle);
                return;
            }
            if (IsOnEdge(sel.Bounds, p))
            {
                Cursor = Cursors.SizeAll;
                return;
            }
        }

        // Over a committed selection's edge → move cursor too.
        if (HitCommitted(p) >= 0)
        {
            Cursor = Cursors.SizeAll;
            return;
        }

        Cursor = Cursors.Cross;
    }

    /// <summary>
    /// True when p is on the bounding box's border band (within a tolerance of an
    /// edge line), the move zone. The interior is deliberately excluded so a new
    /// selection can be started inside an existing one.
    /// </summary>
    private static bool IsOnEdge(Rect b, Point p)
    {
        const double edgeTol = 8;
        if (!Inflate(b, edgeTol).Contains(p))
            return false;

        var inner = Deflate(b, edgeTol);
        return inner.IsEmpty || !inner.Contains(p);
    }

    private static Rect Deflate(Rect r, double by)
    {
        var w = r.Width - 2 * by;
        var h = r.Height - 2 * by;
        return w <= 0 || h <= 0 ? Rect.Empty : new Rect(r.X + by, r.Y + by, w, h);
    }

    private static Cursor CursorForHandle(Handle h) => h switch
    {
        Handle.TopLeft or Handle.BottomRight => Cursors.SizeNWSE,
        Handle.TopRight or Handle.BottomLeft => Cursors.SizeNESW,
        Handle.Top or Handle.Bottom => Cursors.SizeNS,
        Handle.Left or Handle.Right => Cursors.SizeWE,
        _ => Cursors.Cross,
    };

    // ---- Rendering -------------------------------------------------------------

    private IEnumerable<Selection> AllSelections()
    {
        foreach (var c in _committed)
            yield return c;
        if (_selection is { Bounds.Width: > 0, Bounds.Height: > 0 } s)
            yield return s;
    }

    private void UpdateVisuals()
    {
        var hasAny = _committed.Count > 0 || _selection is { Bounds.Width: > 0, Bounds.Height: > 0 };
        if (hasAny != _selectionActive)
        {
            _selectionActive = hasAny;
            SelectionActiveChanged?.Invoke(hasAny);
        }

        var full = new Rect(0, 0, RootGrid.ActualWidth, RootGrid.ActualHeight);
        var dimGeo = new GeometryGroup { FillRule = FillRule.EvenOdd };
        dimGeo.Children.Add(new RectangleGeometry(full));

        // Polygon being drawn: keep the whole screen dim, show the outline-in-progress.
        if (_polygonInProgress && _polygonPoints.Count > 0)
        {
            DimPath.Data = dimGeo;
            RenderPolygonPreview();
            SelectionOutline.Visibility = Visibility.Collapsed;
            HideToolbar();
            PositionHandles(default, show: false);
            SizeBadge.Visibility = Visibility.Collapsed;
            return;
        }
        PolygonPreview.Visibility = Visibility.Collapsed;

        if (hasAny)
        {
            // Every selection punches a hole in the dim and gets an outline.
            var outlineGeo = new GeometryGroup();
            foreach (var s in AllSelections())
            {
                var g = s.BuildGeometry();
                dimGeo.Children.Add(g);
                outlineGeo.Children.Add(g);
            }
            DimPath.Data = dimGeo;
            SelectionOutline.Data = outlineGeo;
            SelectionOutline.Visibility = Visibility.Visible;

            // Handles + toolbar ride the active selection only.
            var anchor = _selection ?? _committed[^1];
            var showHandles = _selection is { } && _dragMode != DragMode.NewSelection;
            PositionHandles(anchor.Bounds, showHandles);

            // Unlike the toolbar, the badge stays up while dragging: sizing the
            // selection is exactly when the number is worth reading.
            PositionSizeBadge(anchor.Bounds);

            if (_dragMode != DragMode.None)
                HideToolbar();
            else
                PositionToolbar(anchor.Bounds);
        }
        else
        {
            DimPath.Data = dimGeo;
            SelectionOutline.Visibility = Visibility.Collapsed;
            HideToolbar();
            PositionHandles(default, show: false);
            SizeBadge.Visibility = Visibility.Collapsed;
        }

        // Re-clip existing strokes when the selection changes.
        if (_document is { HasPaint: true })
            RefreshDrawSurface();
    }

    /// <summary>Draws the committed polygon edges plus a rubber line to the cursor.</summary>
    private void RenderPolygonPreview()
    {
        var figure = new PathFigure { StartPoint = _polygonPoints[0], IsClosed = false };
        for (var i = 1; i < _polygonPoints.Count; i++)
            figure.Segments.Add(new LineSegment(_polygonPoints[i], true));
        figure.Segments.Add(new LineSegment(_polygonCursor, true));

        var geo = new PathGeometry();
        geo.Figures.Add(figure);
        PolygonPreview.Data = geo;
        PolygonPreview.Visibility = Visibility.Visible;
    }

    private void PositionHandles(Rect sel, bool show)
    {
        foreach (var (handle, shape) in _handleShapes)
        {
            if (!show)
            {
                shape.Visibility = Visibility.Collapsed;
                continue;
            }

            var c = HandleCenter(handle, sel);
            Canvas.SetLeft(shape, c.X - HandleSize / 2);
            Canvas.SetTop(shape, c.Y - HandleSize / 2);
            shape.Visibility = Visibility.Visible;
        }
    }

    /// <summary>
    /// Shows what the selection will export as, in physical pixels, just outside its
    /// top-left corner; with no room above (the selection reaches the top of the
    /// screen) it tucks inside instead, the same way the toolbar does at the bottom.
    /// The offsets clear the corner handle, which is drawn centred on that corner.
    /// </summary>
    private void PositionSizeBadge(Rect sel)
    {
        // The reading has to be the exported size, not the DIP box: on a scaled
        // display those differ, and the number is only useful if it matches the file.
        var px = BoundsToPixels(sel);
        SizeBadgeText.Text = $"{px.Width} × {px.Height}";

        SizeBadge.Visibility = Visibility.Visible;
        SizeBadge.UpdateLayout(); // the text just changed, so the width has too

        const double gap = 6;
        var bw = SizeBadge.ActualWidth;
        var bh = SizeBadge.ActualHeight;

        var x = sel.Left;
        var y = sel.Top - bh - gap;

        if (y < 0)
        {
            x = sel.Left + gap;
            y = sel.Top + gap;
        }

        Canvas.SetLeft(SizeBadge, Math.Clamp(x, 0, Math.Max(0, RootGrid.ActualWidth - bw)));
        Canvas.SetTop(SizeBadge, Math.Clamp(y, 0, Math.Max(0, RootGrid.ActualHeight - bh)));
    }

    /// <summary>
    /// Places the toolbar just outside the selection's bottom-right corner; if there
    /// is no room below (selection reaches the screen edge), it tucks inside instead
    /// (SPEC §5). Positions are clamped to stay fully on-screen.
    /// </summary>
    private void PositionToolbar(Rect sel)
    {
        Toolbar.Visibility = Visibility.Visible;
        Toolbar.UpdateLayout();

        var tw = Toolbar.ActualWidth;
        var th = Toolbar.ActualHeight;
        const double margin = 8;

        var x = sel.Right - tw;
        var y = sel.Bottom + margin;

        if (y + th > RootGrid.ActualHeight)
            y = sel.Bottom - th - margin;

        x = Math.Clamp(x, 0, Math.Max(0, RootGrid.ActualWidth - tw));
        y = Math.Clamp(y, 0, Math.Max(0, RootGrid.ActualHeight - th));

        Canvas.SetLeft(Toolbar, x);
        Canvas.SetTop(Toolbar, y);

        UpdateToolSettingsUi();
    }

    /// <summary>Hides the toolbar and everything anchored to it.</summary>
    private void HideToolbar()
    {
        Toolbar.Visibility = Visibility.Collapsed;
        SettingsStrip.Visibility = Visibility.Collapsed;
        CloseFlyouts();

        // The panel is anchored to a toolbar that is no longer there, so it goes at
        // once, animating it out against a vanished anchor would just look broken.
        _brushPanelOpen = false;
        _panelAnimating = false;
        BrushPanel.BeginAnimation(Canvas.TopProperty, null);
        BrushPanel.BeginAnimation(HeightProperty, null);
        SettingsStrip.BeginAnimation(Canvas.TopProperty, null);
        BrushPanel.Visibility = Visibility.Collapsed;
    }

    // ---- Export ----------------------------------------------------------------

    private enum ExportKind { Copy, Save, SaveAs }

    private void ExportAndClose(ExportKind kind)
    {
        CommitText(); // bake any unfinished text into the document first
        Log.Info($"Export requested: {kind}; shape={_selection?.Kind}; bounds={_selection?.Bounds}.");
        if (!TryGetExportImage(out var image))
        {
            Log.Warn("Export: no valid selection to export.");
            return;
        }

        switch (kind)
        {
            case ExportKind.Copy:
                _exporter.CopyToClipboard(image);
                EndSession(produced: true);
                break;

            case ExportKind.Save:
                _exporter.Save(image);
                EndSession(produced: true);
                break;

            case ExportKind.SaveAs:
                if (_exporter.SaveAs(image, this) is not null)
                    EndSession(produced: true);
                break;
        }
    }

    private bool TryGetExportImage(out BitmapSource image)
    {
        image = null!;
        if (_frameBitmap is null)
            return false;

        var selections = AllSelections().ToList();
        if (selections.Count == 0)
            return false;

        // Physical-pixel bounds of each region, and their union (SPEC O-1).
        var rects = selections.Select(s => BoundsToPixels(s.Bounds)).ToList();
        var union = UnionRects(rects);
        if (union.Width <= 0 || union.Height <= 0)
            return false;

        // Combined mask in union-local pixels: keep every shape, transparent between.
        var mask = new GeometryGroup();
        for (var i = 0; i < selections.Count; i++)
        {
            var r = rects[i];
            var local = new Rect(r.X - union.X, r.Y - union.Y, r.Width, r.Height);
            mask.Children.Add(selections[i].BuildGeometry(local));
        }

        // Use the full composite (base + effects + paint + vectors + absolute-erase)
        // as the source, then mask by the selection shape.
        BitmapSource source;
        if (_document is { HasContent: true })
        {
            EnsureBaseBitmap();
            using var composite = _document.RenderComposite(_baseBitmap!);
            source = SkiaInterop.ToBitmapSource(composite);
        }
        else
        {
            source = _frameBitmap;
        }

        image = CaptureExporter.CropMasked(source, union, mask);
        return true;
    }

    /// <summary>Selection bounds (DIP) → physical pixels, clamped to the frame.</summary>
    private Int32Rect BoundsToPixels(Rect b)
    {
        var x = (int)Math.Round(b.X * _dpiScaleX);
        var y = (int)Math.Round(b.Y * _dpiScaleY);
        var w = (int)Math.Round(b.Width * _dpiScaleX);
        var h = (int)Math.Round(b.Height * _dpiScaleY);
        return ClampToFrame(new Int32Rect(x, y, Math.Max(0, w), Math.Max(0, h)));
    }

    private static Int32Rect UnionRects(IReadOnlyList<Int32Rect> rects)
    {
        int minX = int.MaxValue, minY = int.MaxValue, maxR = int.MinValue, maxB = int.MinValue;
        foreach (var r in rects)
        {
            minX = Math.Min(minX, r.X);
            minY = Math.Min(minY, r.Y);
            maxR = Math.Max(maxR, r.X + r.Width);
            maxB = Math.Max(maxB, r.Y + r.Height);
        }
        return new Int32Rect(minX, minY, Math.Max(0, maxR - minX), Math.Max(0, maxB - minY));
    }

    private Int32Rect ClampToFrame(Int32Rect r)
    {
        var x = Math.Clamp(r.X, 0, _frame.Width);
        var y = Math.Clamp(r.Y, 0, _frame.Height);
        var w = Math.Clamp(r.Width, 0, _frame.Width - x);
        var h = Math.Clamp(r.Height, 0, _frame.Height - y);
        return new Int32Rect(x, y, w, h);
    }

    private void EndSession(bool produced)
    {
        SessionEnded?.Invoke(this, produced);
        Close();
    }

    /// <summary>
    /// Left-click Audio: record everything (system + microphone). The recorder's default is
    /// deliberately "capture it all", narrowing it down is what the right-click picker is for.
    /// </summary>
    private void StartAudioWithDefaults()
    {
        AudioRecordRequested?.Invoke(new Reshot.Recording.AudioSources
        {
            SystemFull = true,
            Mic = true,
            MicDevice = _settings.Audio.MicDevice,
        });
        Close();
    }

    /// <summary>Right-click Audio: a source picker (system / mic / specific windows) with QoL.</summary>
    private void ShowAudioMenu() => ShowSourceMenu(AudioTool, "●  Start recording", withMicRow: true, sources =>
    {
        AudioRecordRequested?.Invoke(sources);
        Close();
    });

    /// <summary>
    /// Right-click Record: the same source picker, but it starts a <b>video</b> recording of
    /// the selection with those audio sources (so you can record only one app's sound).
    /// </summary>
    private void ShowVideoAudioMenu() =>
        ShowSourceMenu(RecordTool, "●  Start video recording", withMicRow: false, RequestRecording);

    /// <summary>
    /// Builds the shared audio-source picker, styled like the rest of the HUD. Everything is
    /// checked by default (record all sound); unchecking windows narrows it to per-process
    /// loopback. <paramref name="withMicRow"/> adds the System/Microphone rows (the audio
    /// recorder); without it the menu is just "Select all" plus the window list, and the
    /// microphone follows the video settings.
    /// </summary>
    private void ShowSourceMenu(
        Button target, string startLabel, bool withMicRow, Action<Reshot.Recording.AudioSources> onStart)
    {
        var menu = new System.Windows.Controls.ContextMenu
        {
            Style = (Style)FindResource("HlContextMenu"),
            ItemContainerStyle = (Style)FindResource("HlMenuItem"),
        };
        var separatorStyle = (Style)FindResource("HlSeparator");
        var syncing = false;

        // "System audio" for the recorder, "Select all" for video, same toggle either way:
        // it selects every window, which is what the full system mix is.
        var system = new MenuItem
        {
            Header = withMicRow ? "System audio (all windows)" : "Select all",
            IsCheckable = true, StaysOpenOnClick = true, IsChecked = true,
        };
        menu.Items.Add(system);

        MenuItem? mic = null;
        if (withMicRow)
        {
            mic = new MenuItem
            {
                Header = "Microphone", IsCheckable = true, StaysOpenOnClick = true, IsChecked = true,
            };
            menu.Items.Add(mic);
        }
        menu.Items.Add(new System.Windows.Controls.Separator { Style = separatorStyle });

        var windowItems = new List<MenuItem>();
        foreach (var w in Reshot.Recording.WindowEnumerator.List())
        {
            var title = w.Title.Length > 42 ? w.Title[..42] + "…" : w.Title;
            var item = new MenuItem
            {
                Header = $"{w.ProcessName}: {title}",
                IsCheckable = true, StaysOpenOnClick = true, IsChecked = true, Tag = w.ProcessId,
            };
            item.Checked += (_, _) => SyncSystem();
            item.Unchecked += (_, _) => SyncSystem();
            windowItems.Add(item);
            menu.Items.Add(item);
        }

        // QoL: toggling System/Select all (re)selects/deselects every window.
        system.Checked += (_, _) => { if (!syncing) SetAllWindows(true); };
        system.Unchecked += (_, _) => { if (!syncing) SetAllWindows(false); };

        menu.Items.Add(new System.Windows.Controls.Separator { Style = separatorStyle });
        var start = new MenuItem { Header = startLabel };
        start.Click += (_, _) =>
        {
            var pids = windowItems.Where(i => i.IsChecked).Select(i => (int)i.Tag).ToArray();
            var all = windowItems.Count > 0 && pids.Length == windowItems.Count;
            var full = system.IsChecked && (windowItems.Count == 0 || all);
            onStart(new Reshot.Recording.AudioSources
            {
                Mic = mic?.IsChecked ?? _settings.Video.Audio.Mic,
                MicDevice = _settings.Audio.MicDevice,
                SystemFull = full,
                IncludePids = full ? System.Array.Empty<int>() : pids,
            });
        };
        menu.Items.Add(start);

        menu.PlacementTarget = target;
        menu.IsOpen = true;

        void SetAllWindows(bool value)
        {
            syncing = true;
            foreach (var i in windowItems) i.IsChecked = value;
            syncing = false;
        }

        void SyncSystem()
        {
            if (syncing) return;
            syncing = true;
            system.IsChecked = windowItems.Count > 0 && windowItems.All(i => i.IsChecked);
            syncing = false;
        }
    }

    /// <summary>
    /// Starts recording the selection's bounding box: hands the App the rect in
    /// virtual-screen pixels and closes the overlay so the live screen shows.
    /// </summary>
    private void RequestRecording() => RequestRecording(null);

    /// <param name="sources">Audio sources to record with; null = the saved settings.</param>
    private void RequestRecording(Reshot.Recording.AudioSources? sources)
    {
        var selections = AllSelections().ToList();
        if (selections.Count == 0)
            return;

        var rects = selections.Select(s => BoundsToPixels(s.Bounds)).ToList();
        var union = UnionRects(rects);
        if (union.Width < 2 || union.Height < 2)
            return;

        var mask = BuildRecordingMask(selections, rects, union);
        var screenRect = new Int32Rect(
            _frame.VirtualLeft + union.X, _frame.VirtualTop + union.Y, union.Width, union.Height);
        RecordRequested?.Invoke(screenRect, mask, sources);
        Close();
    }

    /// <summary>
    /// Rasterizes the selected shape(s) into an 8-bit coverage mask (union-local,
    /// stride = union.Width; 255 inside, 0 outside). Returns null for a single plain
    /// rectangle that fills the whole union (nothing to mask).
    /// </summary>
    private static byte[]? BuildRecordingMask(
        IReadOnlyList<Selection> selections, IReadOnlyList<Int32Rect> rects, Int32Rect union)
    {
        if (selections.Count == 1 && selections[0].Kind == ShapeKind.Rectangle)
            return null;

        var group = new GeometryGroup { FillRule = FillRule.Nonzero };
        for (var i = 0; i < selections.Count; i++)
        {
            var r = rects[i];
            group.Children.Add(selections[i].BuildGeometry(new Rect(r.X - union.X, r.Y - union.Y, r.Width, r.Height)));
        }

        var visual = new DrawingVisual();
        using (var dc = visual.RenderOpen())
            dc.DrawGeometry(Brushes.White, null, group);

        var rtb = new RenderTargetBitmap(union.Width, union.Height, 96, 96, PixelFormats.Pbgra32);
        rtb.Render(visual);

        var pixels = new byte[union.Width * union.Height * 4];
        rtb.CopyPixels(pixels, union.Width * 4, 0);

        var mask = new byte[union.Width * union.Height];
        for (var i = 0; i < mask.Length; i++)
            mask[i] = pixels[i * 4 + 3]; // alpha = shape coverage
        return mask;
    }

    protected override void OnClosed(EventArgs e)
    {
        _document?.Dispose();
        _strokeClip?.Dispose();
        _effectStroke?.Dispose();
        _baseBitmap?.Dispose();
        _blurBase?.Dispose();
        _pixelBase?.Dispose();
        _checkerShader?.Dispose();
        base.OnClosed(e);
    }
}
