using SkiaSharp;

namespace Reshot.Core.Tools;

/// <summary>Brush appearance (SPEC §6.1): color, thickness, opacity.</summary>
public sealed class BrushSettings
{
    /// <summary>Stroke color (opaque RGB; opacity is applied separately).</summary>
    public SKColor Color { get; set; } = new(0xFF, 0x3B, 0x30); // Reshot red

    /// <summary>Stroke width in physical pixels.</summary>
    public float Thickness { get; set; } = 6f;

    /// <summary>0..1. Applied to the whole stroke, so overlaps don't darken.</summary>
    public float Opacity { get; set; } = 1f;

    /// <summary>
    /// Eraser radial falloff (Photoshop hardness). 1 = the whole disc erases at full
    /// strength (hard edge); lower values fade the erase from 100% at the centre down to
    /// this fraction at the rim, giving a soft gradient. Erasers only.
    /// </summary>
    public float Hardness { get; set; } = 1f;

    /// <summary>Text tool font family. Empty or unknown resolves to the default.</summary>
    public string FontFamily { get; set; } = FontCatalog.DefaultFamily;
}

/// <summary>
/// A single freehand brush stroke. Points accumulate into one path so the stroke
/// is painted exactly once on commit, overlapping parts of a semi-transparent
/// stroke don't build up (Photoshop-like). Pure Skia, no UI.
/// </summary>
public sealed class BrushStroke
{
    private readonly SKPath _path = new();
    private bool _started;

    public BrushSettings Settings { get; }

    public BrushStroke(BrushSettings settings) => Settings = settings;

    public bool IsEmpty => !_started;

    public void Begin(SKPoint p)
    {
        _path.MoveTo(p);
        _started = true;
    }

    public void Extend(SKPoint p)
    {
        if (_started)
            _path.LineTo(p);
    }

    /// <summary>The stroke path so far (for live preview or commit).</summary>
    public SKPath Path => _path;

    /// <summary>Builds the paint for this stroke (round cap/join, anti-aliased).</summary>
    public SKPaint CreatePaint()
    {
        var color = Settings.Color.WithAlpha((byte)Math.Round(Math.Clamp(Settings.Opacity, 0, 1) * 255));
        return new SKPaint
        {
            Color = color,
            IsAntialias = true,
            Style = SKPaintStyle.Stroke,
            StrokeWidth = Settings.Thickness,
            StrokeCap = SKStrokeCap.Round,
            StrokeJoin = SKStrokeJoin.Round,
        };
    }
}
