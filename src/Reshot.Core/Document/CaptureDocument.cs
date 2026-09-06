using Reshot.Core.Tools;
using SkiaSharp;

namespace Reshot.Core.Document;

/// <summary>
/// The editable state of a capture session (ARCHITECTURE §3). Layers, bottom→top:
/// EffectsLayer (blur/pixelize over the untouched base), PaintLayer (brush strokes
/// plus rasterized shapes/text, vectors bake to permanent ink on commit). Plus an
/// AbsoluteMask that punches the whole composite (incl. the base frame) to
/// transparency. Sized in physical pixels; Skia only.
/// </summary>
public sealed class CaptureDocument : IDisposable
{
    private SKBitmap? _paintLayer;
    private SKBitmap? _effectsLayer;
    private SKBitmap? _absoluteMask;
    private SKCanvas? _paintCanvas;
    private SKCanvas? _effectsCanvas;
    private SKCanvas? _absoluteCanvas;
    private bool _disposed;

    public int Width { get; }
    public int Height { get; }

    /// <summary>Blur/pixelize results over the base, transparent where untouched.</summary>
    public SKBitmap EffectsLayer => EnsureEffectsLayer();

    /// <summary>Permanent brush strokes and rasterized shapes/text, transparent where nothing was drawn.</summary>
    public SKBitmap PaintLayer => EnsurePaintLayer();

    /// <summary>Coverage of the Absolute Eraser (opaque = punch to transparency on export).</summary>
    public SKBitmap AbsoluteMask => EnsureAbsoluteMask();

    public bool HasPaint { get; private set; }
    public bool HasEffects { get; private set; }
    public bool HasAbsolute { get; private set; }

    public CaptureDocument(int width, int height)
    {
        Width = width;
        Height = height;
    }

    private static SKBitmap NewLayer(int w, int h)
    {
        // Skia allocates zero-filled pixel memory, so a fresh premultiplied bitmap is already
        // transparent. Avoiding an explicit erase saves a full-frame memset on every layer.
        return new SKBitmap(new SKImageInfo(w, h, SKColorType.Bgra8888, SKAlphaType.Premul));
    }

    private SKBitmap EnsureLayer(ref SKBitmap? layer, ref SKCanvas? canvas)
    {
        ObjectDisposedException.ThrowIf(_disposed, this);
        if (layer is null)
        {
            layer = NewLayer(Width, Height);
            canvas = new SKCanvas(layer);
        }

        return layer;
    }

    private SKBitmap EnsurePaintLayer() => EnsureLayer(ref _paintLayer, ref _paintCanvas);

    private SKBitmap EnsureEffectsLayer() => EnsureLayer(ref _effectsLayer, ref _effectsCanvas);

    private SKBitmap EnsureAbsoluteMask() => EnsureLayer(ref _absoluteMask, ref _absoluteCanvas);

    private SKCanvas EnsurePaintCanvas()
    {
        EnsurePaintLayer();
        return _paintCanvas!;
    }

    private SKCanvas EnsureEffectsCanvas()
    {
        EnsureEffectsLayer();
        return _effectsCanvas!;
    }

    private SKCanvas EnsureAbsoluteCanvas()
    {
        EnsureAbsoluteMask();
        return _absoluteCanvas!;
    }

    // ---- Brush -----------------------------------------------------------------

    public void CommitStroke(SKPath stroke, SKPaint paint, SKPath? clip)
    {
        var canvas = EnsurePaintCanvas();
        canvas.Save();
        if (clip is not null)
            canvas.ClipPath(clip, antialias: true);
        canvas.DrawPath(stroke, paint);
        canvas.Restore();
        HasPaint = true;
    }

    /// <summary>
    /// Rasterizes a finished vector object (shape, text) into the paint layer.
    /// Vectors are baked to permanent ink on commit, so the eraser treats them
    /// exactly like brush strokes (pixel-wise, never whole-object).
    /// </summary>
    public void CommitVector(VectorObject v, SKPath? clip)
    {
        var canvas = EnsurePaintCanvas();
        canvas.Save();
        if (clip is not null)
            canvas.ClipPath(clip, antialias: true);
        v.Draw(canvas);
        canvas.Restore();
        HasPaint = true;
    }

    // ---- Effects & erasers -----------------------------------------------------

    /// <summary>Stamps effect-source pixels (blurred/pixelized base) into the area.</summary>
    public void ApplyEffect(SKBitmap source, SKPath area, SKPath? selectionClip)
    {
        var canvas = EnsureEffectsCanvas();
        canvas.Save();
        if (selectionClip is not null)
            canvas.ClipPath(selectionClip, antialias: true);
        canvas.ClipPath(area, antialias: true);
        canvas.DrawBitmap(source, 0, 0);
        canvas.Restore();
        HasEffects = true;
    }

    /// <summary>Filter Eraser: clears the effects layer in the area (returns the original).</summary>
    public void EraseEffects(SKPath area, SKPath? selectionClip) =>
        ClearArea(EnsureEffectsCanvas(), area, selectionClip);

    /// <summary>Eraser: clears painted pixels in the area.</summary>
    public void ErasePaint(SKPath area, SKPath? selectionClip) =>
        ClearArea(EnsurePaintCanvas(), area, selectionClip);

    /// <summary>Absolute Eraser: marks the area to be punched to transparency on export.</summary>
    public void AbsoluteErase(SKPath area, SKPath? selectionClip)
    {
        var canvas = EnsureAbsoluteCanvas();
        canvas.Save();
        if (selectionClip is not null)
            canvas.ClipPath(selectionClip, antialias: true);
        canvas.ClipPath(area, antialias: true);
        using var paint = new SKPaint { Color = SKColors.White };
        canvas.DrawPaint(paint); // DrawPaint respects the clip (Clear does not)
        canvas.Restore();
        HasAbsolute = true;
    }

    private static void ClearArea(SKCanvas canvas, SKPath area, SKPath? selectionClip)
    {
        canvas.Save();
        if (selectionClip is not null)
            canvas.ClipPath(selectionClip, antialias: true);
        canvas.ClipPath(area, antialias: true);
        using var paint = new SKPaint { BlendMode = SKBlendMode.Clear };
        canvas.DrawPaint(paint); // clears only within the clip
        canvas.Restore();
    }

    // ---- Soft (Photoshop-style) erasers ---------------------------------------
    //
    // A "coverage" bitmap is an opaque greyscale image whose luminance is the per-pixel
    // erase strength (0 = keep, 1 = fully erase), with the eraser's Global-opacity already
    // baked in. CreateCoverageAlphaFilter turns that luminance into a pure alpha mask so a
    // single DstOut removes exactly that fraction of the layer, no compounding.

    /// <summary>Maps a greyscale coverage bitmap to a pure-alpha mask (alpha = red, rgb = 0).</summary>
    public static SKColorFilter CreateCoverageAlphaFilter() => SKColorFilter.CreateColorMatrix(new float[]
    {
        0, 0, 0, 0, 0,
        0, 0, 0, 0, 0,
        0, 0, 0, 0, 0,
        1, 0, 0, 0, 0,
    });

    /// <summary>Eraser: removes painted pixels by the coverage strength (soft edge).</summary>
    public void ErasePaintCoverage(SKBitmap coverage, int dx, int dy, SKPath? selectionClip) =>
        ApplyCoverage(EnsurePaintCanvas(), coverage, dx, dy, selectionClip, SKBlendMode.DstOut);

    /// <summary>Filter Eraser: removes effect pixels by the coverage strength (soft edge).</summary>
    public void EraseEffectsCoverage(SKBitmap coverage, int dx, int dy, SKPath? selectionClip) =>
        ApplyCoverage(EnsureEffectsCanvas(), coverage, dx, dy, selectionClip, SKBlendMode.DstOut);

    /// <summary>Absolute Eraser: accumulates coverage into the punch-through mask (soft edge).</summary>
    public void AbsoluteEraseCoverage(SKBitmap coverage, int dx, int dy, SKPath? selectionClip)
    {
        ApplyCoverage(EnsureAbsoluteCanvas(), coverage, dx, dy, selectionClip, SKBlendMode.SrcOver);
        HasAbsolute = true;
    }

    private static void ApplyCoverage(SKCanvas canvas, SKBitmap coverage, int dx, int dy,
        SKPath? selectionClip, SKBlendMode blend)
    {
        canvas.Save();
        if (selectionClip is not null)
            canvas.ClipPath(selectionClip, antialias: true);
        using var filter = CreateCoverageAlphaFilter();
        using var paint = new SKPaint { BlendMode = blend, ColorFilter = filter };
        canvas.DrawBitmap(coverage, dx, dy, paint);
        canvas.Restore();
    }

    // ---- Undo snapshots (generic over any layer) -------------------------------

    /// <summary>Copies a rectangular region of a layer for an undo snapshot.</summary>
    public static SKBitmap SnapshotRegion(SKBitmap layer, SKRectI region)
    {
        var clipped = ClipRegion(layer, region);
        var width = Math.Max(1, clipped.Width);
        var height = Math.Max(1, clipped.Height);
        var snap = new SKBitmap(new SKImageInfo(width, height, SKColorType.Bgra8888, SKAlphaType.Premul));
        using var canvas = new SKCanvas(snap);
        canvas.Clear(SKColors.Transparent);
        if (!clipped.IsEmpty)
        {
            canvas.DrawBitmap(
                layer,
                SKRect.Create(clipped.Left, clipped.Top, clipped.Width, clipped.Height),
                SKRect.Create(0, 0, clipped.Width, clipped.Height));
        }
        return snap;
    }

    /// <summary>Overwrites a region of a layer with a snapshot (undo/redo).</summary>
    public static void RestoreRegion(SKBitmap layer, SKRectI region, SKBitmap snapshot)
    {
        var clipped = ClipRegion(layer, region);
        if (clipped.IsEmpty)
            return;

        using var canvas = new SKCanvas(layer);
        using var paint = new SKPaint { BlendMode = SKBlendMode.Src };
        var sourceX = snapshot.Width == region.Width ? clipped.Left - region.Left : 0;
        var sourceY = snapshot.Height == region.Height ? clipped.Top - region.Top : 0;
        var width = Math.Min(clipped.Width, snapshot.Width - sourceX);
        var height = Math.Min(clipped.Height, snapshot.Height - sourceY);
        if (width <= 0 || height <= 0)
            return;
        canvas.DrawBitmap(
            snapshot,
            SKRect.Create(sourceX, sourceY, width, height),
            SKRect.Create(clipped.Left, clipped.Top,
                width, height),
            paint);
    }

    private static SKRectI ClipRegion(SKBitmap layer, SKRectI region)
    {
        var left = Math.Clamp(region.Left, 0, layer.Width);
        var top = Math.Clamp(region.Top, 0, layer.Height);
        var right = Math.Clamp(region.Right, 0, layer.Width);
        var bottom = Math.Clamp(region.Bottom, 0, layer.Height);
        return right > left && bottom > top
            ? new SKRectI(left, top, right, bottom)
            : SKRectI.Empty;
    }

    // ---- Composition -----------------------------------------------------------

    public bool HasContent => HasPaint || HasEffects || HasAbsolute;

    /// <summary>
    /// Full export composite: base → effects → paint, then the Absolute
    /// Eraser punches the whole thing (incl. the base) to transparency.
    /// </summary>
    public SKBitmap RenderComposite(SKBitmap baseBitmap)
    {
        var bmp = new SKBitmap(new SKImageInfo(Width, Height, SKColorType.Bgra8888, SKAlphaType.Premul));
        using var canvas = new SKCanvas(bmp);
        canvas.Clear(SKColors.Transparent);
        canvas.DrawBitmap(baseBitmap, 0, 0);
        if (_effectsLayer is not null)
            canvas.DrawBitmap(_effectsLayer, 0, 0);
        if (_paintLayer is not null)
            canvas.DrawBitmap(_paintLayer, 0, 0);

        if (HasAbsolute && _absoluteMask is not null)
        {
            using var punch = new SKPaint { BlendMode = SKBlendMode.DstOut };
            canvas.DrawBitmap(_absoluteMask, 0, 0, punch);
        }
        return bmp;
    }

    /// <summary>
    /// Composites effects → paint into one frame-sized overlay bitmap
    /// (transparent where empty). The base frame and the absolute mask are applied
    /// by the exporter around this.
    /// </summary>
    public SKBitmap RenderOverlay()
    {
        var bmp = new SKBitmap(new SKImageInfo(Width, Height, SKColorType.Bgra8888, SKAlphaType.Premul));
        using var canvas = new SKCanvas(bmp);
        canvas.Clear(SKColors.Transparent);
        if (_effectsLayer is not null)
            canvas.DrawBitmap(_effectsLayer, 0, 0);
        if (_paintLayer is not null)
            canvas.DrawBitmap(_paintLayer, 0, 0);
        return bmp;
    }

    public void Dispose()
    {
        if (_disposed)
            return;
        _disposed = true;
        _paintCanvas?.Dispose();
        _effectsCanvas?.Dispose();
        _absoluteCanvas?.Dispose();
        _paintLayer?.Dispose();
        _effectsLayer?.Dispose();
        _absoluteMask?.Dispose();
    }
}
