using System.Diagnostics;
using System.Reflection;
using Reshot.Core.Document;
using SkiaSharp;
using Xunit;
using Xunit.Abstractions;

namespace Reshot.Core.Tests;

public sealed class CaptureDocumentPerfTests
{
    private readonly ITestOutputHelper _output;

    public CaptureDocumentPerfTests(ITestOutputHelper output) => _output = output;

    [Fact]
    public void Construction_and_empty_renders_do_not_realize_layers()
    {
        using var doc = new CaptureDocument(64, 48);

        Assert.Null(GetField(doc, "_paintLayer"));
        Assert.Null(GetField(doc, "_effectsLayer"));
        Assert.Null(GetField(doc, "_absoluteMask"));

        using var overlay = doc.RenderOverlay();
        Assert.Null(GetField(doc, "_paintLayer"));
        Assert.Null(GetField(doc, "_effectsLayer"));
        Assert.Null(GetField(doc, "_absoluteMask"));

        using var frame = new SKBitmap(new SKImageInfo(64, 48, SKColorType.Bgra8888, SKAlphaType.Opaque));
        using var composite = doc.RenderComposite(frame);
        Assert.Equal(frame.GetPixel(10, 10), composite.GetPixel(10, 10));
        Assert.Null(GetField(doc, "_paintLayer"));
        Assert.Null(GetField(doc, "_effectsLayer"));
        Assert.Null(GetField(doc, "_absoluteMask"));
    }

    [Fact]
    public void A_stroke_realizes_only_the_paint_layer()
    {
        using var doc = new CaptureDocument(64, 48);
        using var path = new SKPath();
        path.MoveTo(4, 4);
        path.LineTo(20, 20);
        using var paint = new SKPaint { Color = SKColors.Red, StrokeWidth = 4, Style = SKPaintStyle.Stroke };

        doc.CommitStroke(path, paint, null);

        Assert.NotNull(GetField(doc, "_paintLayer"));
        Assert.Null(GetField(doc, "_effectsLayer"));
        Assert.Null(GetField(doc, "_absoluteMask"));
        Assert.True(doc.HasPaint);
        Assert.True(doc.PaintLayer.GetPixel(10, 10).Alpha > 0);
    }

    [Fact]
    public void Allocation_profile_reports_eager_and_lazy_capture_costs()
    {
        const int width = 3840;
        const int height = 2160;
        const long layerBytes = (long)width * height * 4;

        var eagerEmptyManaged = MeasureManagedAllocation(() =>
        {
            using var paint = EagerLayer(width, height);
            using var effects = EagerLayer(width, height);
            using var absolute = EagerLayer(width, height);
            using var paintCanvas = new SKCanvas(paint);
            using var effectsCanvas = new SKCanvas(effects);
            using var absoluteCanvas = new SKCanvas(absolute);
        });
        var lazyEmptyManaged = MeasureManagedAllocation(() =>
        {
            using var doc = new CaptureDocument(width, height);
        });

        using var strokePath = new SKPath();
        strokePath.MoveTo(4, 4);
        strokePath.LineTo(20, 20);
        using var strokePaint = new SKPaint
        {
            Color = SKColors.Red,
            StrokeWidth = 4,
            Style = SKPaintStyle.Stroke,
        };
        var eagerStrokeManaged = MeasureManagedAllocation(() =>
        {
            using var paint = EagerLayer(width, height);
            using var effects = EagerLayer(width, height);
            using var absolute = EagerLayer(width, height);
            using var paintCanvas = new SKCanvas(paint);
            using var effectsCanvas = new SKCanvas(effects);
            using var absoluteCanvas = new SKCanvas(absolute);
            paintCanvas.DrawPath(strokePath, strokePaint);
        });
        var lazyStrokeManaged = MeasureManagedAllocation(() =>
        {
            using var doc = new CaptureDocument(width, height);
            doc.CommitStroke(strokePath, strokePaint, null);
        });
        // Both paths allocate identical 4K layers; this isolates the redundant erase pass from
        // the allocation itself. The timing is diagnostic only because native allocator state and
        // other processes make a single wall-clock sample noisy.
        _ = MeasureLayerSetupMilliseconds(erase: true);
        _ = MeasureLayerSetupMilliseconds(erase: false);
        var eraseMs = MeasureLayerSetupMilliseconds(erase: true);
        var noEraseMs = MeasureLayerSetupMilliseconds(erase: false);

        _output.WriteLine($"3840x2160 layer bytes: {layerBytes:N0}");
        _output.WriteLine($"empty capture resident layer bytes: eager {layerBytes * 3:N0}, lazy {0:N0}");
        _output.WriteLine($"single-stroke resident layer bytes: eager {layerBytes * 3:N0}, lazy {layerBytes:N0}");
        _output.WriteLine($"GC.GetAllocatedBytesForCurrentThread empty: eager {eagerEmptyManaged:N0}, lazy {lazyEmptyManaged:N0}");
        _output.WriteLine($"GC.GetAllocatedBytesForCurrentThread single stroke: eager {eagerStrokeManaged:N0}, lazy {lazyStrokeManaged:N0}");
        _output.WriteLine($"4K setup wall time (3 layers): with erase {eraseMs:F2} ms, without erase {noEraseMs:F2} ms, delta {eraseMs - noEraseMs:F2} ms");
    }

    private static SKBitmap EagerLayer(int width, int height)
    {
        var bitmap = FreshLayer(width, height);
        bitmap.Erase(SKColors.Transparent);
        return bitmap;
    }

    private static SKBitmap FreshLayer(int width, int height) =>
        new(new SKImageInfo(width, height, SKColorType.Bgra8888, SKAlphaType.Premul));

    private static double MeasureLayerSetupMilliseconds(bool erase)
    {
        var start = Stopwatch.GetTimestamp();
        using var paint = erase ? EagerLayer(3840, 2160) : FreshLayer(3840, 2160);
        using var effects = erase ? EagerLayer(3840, 2160) : FreshLayer(3840, 2160);
        using var absolute = erase ? EagerLayer(3840, 2160) : FreshLayer(3840, 2160);
        return Stopwatch.GetElapsedTime(start).TotalMilliseconds;
    }

    private static long MeasureManagedAllocation(Action action)
    {
        GC.Collect();
        GC.WaitForPendingFinalizers();
        GC.Collect();
        var before = GC.GetAllocatedBytesForCurrentThread();
        action();
        return GC.GetAllocatedBytesForCurrentThread() - before;
    }

    private static object? GetField(CaptureDocument doc, string name) =>
        typeof(CaptureDocument).GetField(name, BindingFlags.Instance | BindingFlags.NonPublic)?.GetValue(doc);
}
