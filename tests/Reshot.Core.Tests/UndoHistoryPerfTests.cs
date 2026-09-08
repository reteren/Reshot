using System.Diagnostics;
using System.Reflection;
using System.Runtime.InteropServices;
using Reshot.Core.Document;
using Reshot.Core.History;
using SkiaSharp;
using Xunit;
using Xunit.Abstractions;

namespace Reshot.Core.Tests;

public sealed class UndoHistoryPerfTests
{
    private readonly ITestOutputHelper _output;

    public UndoHistoryPerfTests(ITestOutputHelper output) => _output = output;

    [Fact]
    public void Eviction_disposes_snapshots_but_never_the_live_layer()
    {
        using var layer = new SKBitmap(new SKImageInfo(8, 8, SKColorType.Bgra8888, SKAlphaType.Premul));
        using (var canvas = new SKCanvas(layer))
            canvas.Clear(SKColors.Red);

        using var history = new UndoHistory();
        var evicted = CreateCommand(layer, 1);
        history.Push(evicted);
        var heldBeforeEviction = evicted.RetainedBytes;
        for (var i = 2; i <= 33; i++)
            history.Push(CreateCommand(layer, i));

        Assert.True(heldBeforeEviction > 0);
        Assert.Equal(0, evicted.RetainedBytes);
        Assert.Equal(IntPtr.Zero, GetBitmap(evicted, "_before").Handle);
        Assert.Equal(IntPtr.Zero, GetBitmap(evicted, "_after").Handle);
        Assert.Throws<ObjectDisposedException>(() => evicted.Undo());
        Assert.Throws<ObjectDisposedException>(() => evicted.Redo());
        Assert.Equal(SKColors.Red, layer.GetPixel(4, 4));
        Assert.True(history.CanUndo);
    }

    [Fact]
    public void Clearing_redo_and_disposing_history_release_command_state()
    {
        using var layer = new SKBitmap(new SKImageInfo(8, 8, SKColorType.Bgra8888, SKAlphaType.Premul));
        var first = CreateCommand(layer, 1);
        var second = CreateCommand(layer, 2);
        var third = CreateCommand(layer, 3);
        using var history = new UndoHistory();

        history.Push(first);
        Assert.True(history.Undo());
        history.Push(second); // Drops first from redo.
        Assert.Equal(IntPtr.Zero, GetBitmap(first, "_before").Handle);
        Assert.Equal(IntPtr.Zero, GetBitmap(first, "_after").Handle);

        history.Push(third);
        history.Dispose();
        Assert.Equal(IntPtr.Zero, GetBitmap(second, "_before").Handle);
        Assert.Equal(IntPtr.Zero, GetBitmap(second, "_after").Handle);
        Assert.Equal(IntPtr.Zero, GetBitmap(third, "_before").Handle);
        Assert.Equal(IntPtr.Zero, GetBitmap(third, "_after").Handle);
        Assert.NotEqual(IntPtr.Zero, layer.Handle);
    }

    [Fact]
    public void Snapshot_region_clips_to_layer_bounds_and_restores_exact_pixels()
    {
        using var layer = new SKBitmap(new SKImageInfo(10, 10, SKColorType.Bgra8888, SKAlphaType.Premul));
        using (var canvas = new SKCanvas(layer))
            canvas.Clear(SKColors.Blue);

        var region = new SKRectI(-2, -2, 6, 6);
        using var snapshot = CaptureDocument.SnapshotRegion(layer, region);
        Assert.Equal(6, snapshot.Width);
        Assert.Equal(6, snapshot.Height);
        Assert.Equal(SKColors.Blue, snapshot.GetPixel(0, 0));

        using (var canvas = new SKCanvas(layer))
            canvas.Clear(SKColors.Transparent);
        CaptureDocument.RestoreRegion(layer, region, snapshot);

        Assert.Equal(SKColors.Blue, layer.GetPixel(0, 0));
        Assert.Equal(SKColors.Blue, layer.GetPixel(5, 5));
        Assert.Equal(0, layer.GetPixel(6, 6).Alpha);

        var commandBefore = CaptureDocument.SnapshotRegion(layer, region);
        using (var canvas = new SKCanvas(layer))
            canvas.Clear(SKColors.Transparent);
        var commandAfter = CaptureDocument.SnapshotRegion(layer, region);
        using var clippedCommand = new LayerRegionCommand(layer, region, commandBefore, commandAfter);
        clippedCommand.Undo();
        Assert.Equal(SKColors.Blue, layer.GetPixel(0, 0));
        clippedCommand.Redo();
        Assert.Equal(0, layer.GetPixel(0, 0).Alpha);
    }

    [Fact]
    public void Sparse_delta_undo_and_redo_are_pixel_exact()
    {
        using var layer = new SKBitmap(new SKImageInfo(64, 64, SKColorType.Bgra8888, SKAlphaType.Premul));
        var before = CaptureDocument.SnapshotRegion(layer, new SKRectI(0, 0, 64, 64));
        using (var canvas = new SKCanvas(layer))
        {
            using var paint = new SKPaint { Color = SKColors.Red, StrokeWidth = 4, Style = SKPaintStyle.Stroke };
            canvas.DrawLine(8, 8, 20, 20, paint);
        }
        var after = CaptureDocument.SnapshotRegion(layer, new SKRectI(0, 0, 64, 64));
        var command = new LayerRegionCommand(layer, new SKRectI(0, 0, 64, 64), before, after);

        var drawn = layer.GetPixel(14, 14);
        Assert.True(command.RetainedBytes < 64L * 64 * 4 * 2);
        command.Undo();
        Assert.Equal(0, layer.GetPixel(14, 14).Alpha);
        command.Redo();
        Assert.Equal(drawn, layer.GetPixel(14, 14));
        command.Dispose();
    }

    [Fact]
    public void Delta_round_trips_empty_single_pixel_and_identical_regions()
    {
        using var layer = NewLayer(8, 8, SKColors.Black);

        var emptyBefore = CaptureDocument.SnapshotRegion(layer, SKRectI.Empty);
        var emptyAfter = CaptureDocument.SnapshotRegion(layer, SKRectI.Empty);
        using var empty = new LayerRegionCommand(layer, SKRectI.Empty, emptyBefore, emptyAfter);
        Assert.Equal(0, empty.RetainedBytes);

        var before = CaptureDocument.SnapshotRegion(layer, new SKRectI(0, 0, 8, 8));
        SetPixel(layer, 3, 4, SKColors.Red);
        var after = CaptureDocument.SnapshotRegion(layer, new SKRectI(0, 0, 8, 8));
        using var single = new LayerRegionCommand(layer, new SKRectI(0, 0, 8, 8), before, after);
        var expected = SnapshotBytes(layer);
        single.Undo();
        Assert.Equal(SKColors.Black, layer.GetPixel(3, 4));
        single.Redo();
        Assert.Equal(expected, SnapshotBytes(layer));

        var sameBefore = CaptureDocument.SnapshotRegion(layer, new SKRectI(0, 0, 8, 8));
        var sameAfter = CaptureDocument.SnapshotRegion(layer, new SKRectI(0, 0, 8, 8));
        using var identical = new LayerRegionCommand(layer, new SKRectI(0, 0, 8, 8), sameBefore, sameAfter);
        Assert.Equal(0, identical.RetainedBytes);
    }

    [Fact]
    public void Fully_changed_and_alternating_regions_fall_back_to_raw_snapshots()
    {
        using var layer = NewLayer(8, 8, SKColors.Black);
        var before = CaptureDocument.SnapshotRegion(layer, new SKRectI(0, 0, 8, 8));
        using (var canvas = new SKCanvas(layer))
            canvas.Clear(SKColors.White);
        var after = CaptureDocument.SnapshotRegion(layer, new SKRectI(0, 0, 8, 8));
        using var full = new LayerRegionCommand(layer, new SKRectI(0, 0, 8, 8), before, after);

        Assert.NotNull(GetBitmapOrNull(full, "_before"));
        Assert.Null(GetBitmapOrNull(full, "_runs"));
        Assert.Equal(8L * 8 * 4 * 2, full.RetainedBytes);
        var fullPixels = SnapshotBytes(layer);
        full.Undo();
        for (var y = 0; y < 8; y++)
        for (var x = 0; x < 8; x++)
            Assert.Equal(SKColors.Black, layer.GetPixel(x, y));
        full.Redo();
        Assert.Equal(fullPixels, SnapshotBytes(layer));

        using var alternatingLayer = NewLayer(8, 8, SKColors.Black);
        var alternatingBefore = CaptureDocument.SnapshotRegion(alternatingLayer, new SKRectI(0, 0, 8, 8));
        for (var y = 0; y < 8; y++)
        for (var x = 0; x < 8; x++)
            if ((x + y) % 2 == 0)
                SetPixel(alternatingLayer, x, y, SKColors.White);
        var alternatingAfter = CaptureDocument.SnapshotRegion(alternatingLayer, new SKRectI(0, 0, 8, 8));
        using var alternating = new LayerRegionCommand(
            alternatingLayer, new SKRectI(0, 0, 8, 8), alternatingBefore, alternatingAfter);

        Assert.Null(GetBitmapOrNull(alternating, "_runs"));
        Assert.Equal(8L * 8 * 4 * 2, alternating.RetainedBytes);
    }

    [Fact]
    public void Lazy_document_layer_round_trips_after_first_materialisation()
    {
        using var document = new CaptureDocument(16, 16);
        var region = new SKRectI(0, 0, 16, 16);
        var before = CaptureDocument.SnapshotRegion(document.PaintLayer, region);
        using (var path = new SKPath())
        {
            path.MoveTo(2, 2);
            path.LineTo(12, 12);
            using var paint = new SKPaint { Color = SKColors.Red, StrokeWidth = 3, Style = SKPaintStyle.Stroke };
            document.CommitStroke(path, paint, null);
        }
        var after = CaptureDocument.SnapshotRegion(document.PaintLayer, region);
        using var command = new LayerRegionCommand(document.PaintLayer, region, before, after);
        var drawn = SnapshotBytes(document.PaintLayer);

        command.Undo();
        Assert.Equal(0, document.PaintLayer.GetPixel(7, 7).Alpha);
        command.Redo();
        Assert.Equal(drawn, SnapshotBytes(document.PaintLayer));
    }

    [Fact]
    public void Eviction_gc_and_finalizers_do_not_touch_the_borrowed_layer()
    {
        using var layer = NewLayer(32, 32, SKColors.Red);
        using var history = new UndoHistory();
        for (var i = 0; i < 64; i++)
            history.Push(CreateCommand(layer, i));

        GC.Collect();
        GC.WaitForPendingFinalizers();
        GC.Collect();

        using (var canvas = new SKCanvas(layer))
            canvas.Clear(SKColors.Blue);
        Assert.Equal(SKColors.Blue, layer.GetPixel(16, 16));
        history.Dispose();
        Assert.Equal(SKColors.Blue, layer.GetPixel(16, 16));
    }

    [Fact]
    public void Evicted_commands_are_not_retained_by_history()
    {
        using var history = new UndoHistory();
        var references = new List<WeakReference>();
        IUndoableCommand? command = null;

        for (var i = 0; i < 5000; i++)
        {
            command = new TrackingCommand();
            references.Add(new WeakReference(command));
            history.Push(command);
        }

        command = null;
        GC.Collect();
        GC.WaitForPendingFinalizers();
        GC.Collect();

        var alive = references.Count(reference => reference.IsAlive);
        Assert.InRange(alive, 1, 32);
    }

    [Fact]
    public void History_serializes_dispose_against_undo_redo_and_disposes_each_command_once()
    {
        using var history = new UndoHistory();
        var commands = Enumerable.Range(0, 40).Select(_ => new TrackingCommand()).ToList();
        foreach (var command in commands)
            history.Push(command);

        Parallel.For(0, 8, _ =>
        {
            for (var i = 0; i < 100; i++)
            {
                try
                {
                    if ((i & 1) == 0)
                        history.Undo();
                    else
                        history.Redo();
                }
                catch (ObjectDisposedException)
                {
                    return;
                }
            }
        });

        history.Dispose();
        history.Dispose();
        Assert.All(commands, command => Assert.Equal(1, command.DisposeCount));
        Assert.Throws<ObjectDisposedException>(() => history.Undo());
        Assert.Throws<ObjectDisposedException>(() => history.Redo());
    }

    [Fact]
    public void History_rejects_reusing_a_command_instance()
    {
        using var history = new UndoHistory();
        var command = new TrackingCommand();

        history.Push(command);
        Assert.Throws<InvalidOperationException>(() => history.Push(command));
        Assert.True(history.Undo());
        Assert.Throws<InvalidOperationException>(() => history.Push(command));

        history.Dispose();
        Assert.Equal(1, command.DisposeCount);
    }

    [Fact]
    public void LayerRegionCommand_rejects_aliasing_its_owned_snapshots()
    {
        using var layer = NewLayer(8, 8, SKColors.Black);
        using var snapshot = CaptureDocument.SnapshotRegion(layer, new SKRectI(0, 0, 8, 8));

        Assert.Throws<ArgumentException>(() => new LayerRegionCommand(
            layer, new SKRectI(0, 0, 8, 8), snapshot, snapshot));
        Assert.NotEqual(IntPtr.Zero, snapshot.Handle);
    }

    [Fact]
    public async Task Dispose_waits_for_an_in_progress_command_and_disposes_it_once()
    {
        using var history = new UndoHistory();
        var entered = new TaskCompletionSource(TaskCreationOptions.RunContinuationsAsynchronously);
        var release = new TaskCompletionSource(TaskCreationOptions.RunContinuationsAsynchronously);
        var command = new BlockingCommand(entered, release);
        history.Push(command);

        var undo = Task.Run(() => history.Undo());
        await entered.Task.WaitAsync(TimeSpan.FromSeconds(5));

        var dispose = Task.Run(history.Dispose);
        Assert.NotSame(dispose, await Task.WhenAny(dispose, Task.Delay(100)));
        Assert.Equal(0, command.DisposeCount);

        release.TrySetResult();
        Assert.True(await undo.WaitAsync(TimeSpan.FromSeconds(5)));
        await dispose.WaitAsync(TimeSpan.FromSeconds(5));
        Assert.Equal(1, command.DisposeCount);
    }

    [Fact]
    public void History_does_not_use_a_borrowed_layer_after_document_disposal()
    {
        var document = new CaptureDocument(8, 8);
        var region = new SKRectI(0, 0, 8, 8);
        var before = CaptureDocument.SnapshotRegion(document.PaintLayer, region);
        SetPixel(document.PaintLayer, 2, 2, SKColors.Red);
        var after = CaptureDocument.SnapshotRegion(document.PaintLayer, region);
        var command = new LayerRegionCommand(document.PaintLayer, region, before, after);
        using var history = new UndoHistory();
        history.Push(command);

        document.Dispose();
        Assert.Throws<ObjectDisposedException>(() => history.Undo());
        Assert.False(history.Redo());
        Assert.Throws<ObjectDisposedException>(() => command.Redo());
    }

    [Fact]
    public void Sparse_delta_round_trips_edge_cases_byte_exactly()
    {
        using var layer = NewLayer(8, 8, SKColors.Black);

        AssertRoundTrip(layer, SKRectI.Empty, _ => { }, expectRaw: false);
        AssertRoundTrip(layer, new SKRectI(0, 0, 8, 8), bitmap =>
            SetPixel(bitmap, 3, 4, SKColors.Red), expectRaw: false);
        AssertRoundTrip(layer, new SKRectI(0, 0, 8, 8), _ => { }, expectRaw: false);
        AssertRoundTrip(layer, new SKRectI(0, 0, 8, 8), bitmap =>
        {
            using var canvas = new SKCanvas(bitmap);
            canvas.Clear(SKColors.White);
        }, expectRaw: true);
        AssertRoundTrip(layer, new SKRectI(0, 0, 8, 8), bitmap =>
        {
            for (var y = 0; y < bitmap.Height; y++)
            for (var x = 0; x < bitmap.Width; x++)
                if ((x + y) % 2 == 0)
                    SetPixel(bitmap, x, y, SKColors.Red);
        }, expectRaw: true);
        AssertRoundTrip(layer, new SKRectI(-2, -2, 6, 6), bitmap =>
            SetPixel(bitmap, 0, 0, SKColors.Blue), expectRaw: false);
    }

    [Fact]
    public void Four_k_snapshot_profile_reports_raw_and_png_costs()
    {
        const int width = 3840;
        const int height = 2160;
        const long rawSnapshotBytes = (long)width * height * 4;
        const long rawCommandBytes = rawSnapshotBytes * 2;

        using var layer = new SKBitmap(new SKImageInfo(width, height, SKColorType.Bgra8888, SKAlphaType.Premul));
        using (var canvas = new SKCanvas(layer))
        {
            canvas.Clear(SKColors.Transparent);
            using var paint = new SKPaint
            {
                Color = SKColors.Red,
                IsAntialias = true,
                StrokeWidth = 8,
                Style = SKPaintStyle.Stroke,
                StrokeCap = SKStrokeCap.Round,
            };
            canvas.DrawLine(32, 32, width - 32, height - 32, paint);
        }

        using var raw = CaptureDocument.SnapshotRegion(layer, new SKRectI(0, 0, width, height));
        var encodeStart = Stopwatch.GetTimestamp();
        using var encoded = raw.Encode(SKEncodedImageFormat.Png, 100);
        var encodeMs = Stopwatch.GetElapsedTime(encodeStart).TotalMilliseconds;
        var decodeStart = Stopwatch.GetTimestamp();
        using var image = SKImage.FromEncodedData(encoded)!;
        using var decoded = SKBitmap.FromImage(image)!;
        var decodeMs = Stopwatch.GetElapsedTime(decodeStart).TotalMilliseconds;

        Assert.NotNull(encoded);
        Assert.Equal(raw.GetPixel(100, 100), decoded.GetPixel(100, 100));
        Assert.Equal(raw.GetPixel(width / 2, height / 2), decoded.GetPixel(width / 2, height / 2));
        Assert.Equal(raw.GetPixel(width - 100, height - 100), decoded.GetPixel(width - 100, height - 100));

        using var emptyLayer = new SKBitmap(new SKImageInfo(width, height, SKColorType.Bgra8888, SKAlphaType.Premul));
        var fullBefore = CaptureDocument.SnapshotRegion(emptyLayer, new SKRectI(0, 0, width, height));
        var fullAfter = CaptureDocument.SnapshotRegion(layer, new SKRectI(0, 0, width, height));
        var deltaStart = Stopwatch.GetTimestamp();
        using var fullDelta = new LayerRegionCommand(
            layer, new SKRectI(0, 0, width, height), fullBefore, fullAfter);
        var deltaCreateMs = Stopwatch.GetElapsedTime(deltaStart).TotalMilliseconds;
        var fullDeltaBytes = fullDelta.RetainedBytes;
        var undoStart = Stopwatch.GetTimestamp();
        fullDelta.Undo();
        var undoMs = Stopwatch.GetElapsedTime(undoStart).TotalMilliseconds;
        var redoStart = Stopwatch.GetTimestamp();
        fullDelta.Redo();
        var redoMs = Stopwatch.GetElapsedTime(redoStart).TotalMilliseconds;

        using var smallLayer = new SKBitmap(new SKImageInfo(64, 64, SKColorType.Bgra8888, SKAlphaType.Premul));
        var smallBefore = CaptureDocument.SnapshotRegion(smallLayer, new SKRectI(0, 0, 64, 64));
        using (var smallCanvas = new SKCanvas(smallLayer))
        {
            using var smallPaint = new SKPaint { Color = SKColors.Red, StrokeWidth = 4, Style = SKPaintStyle.Stroke };
            smallCanvas.DrawLine(8, 8, 56, 56, smallPaint);
        }
        var smallAfter = CaptureDocument.SnapshotRegion(smallLayer, new SKRectI(0, 0, 64, 64));
        using var smallDelta = new LayerRegionCommand(
            smallLayer, new SKRectI(0, 0, 64, 64), smallBefore, smallAfter);
        var smallDeltaBytes = smallDelta.RetainedBytes;

        _output.WriteLine($"4K raw snapshot: {rawSnapshotBytes:N0} bytes; raw command (before+after): {rawCommandBytes:N0} bytes");
        _output.WriteLine($"32-step raw history: {rawCommandBytes * 32:N0} bytes; 1 small 64x64 command: {64L * 64 * 4 * 2:N0} bytes; 32 small commands: {64L * 64 * 4 * 2 * 32:N0} bytes");
        _output.WriteLine($"PNG encoded 4K snapshot: {encoded!.Size:N0} bytes; encode {encodeMs:F2} ms; decode {decodeMs:F2} ms");
        _output.WriteLine($"32-step PNG history at this content: {encoded.Size * 2L * 32:N0} bytes");
        _output.WriteLine($"Sparse delta payload: one 64x64 command {smallDeltaBytes:N0} bytes, 32 commands {smallDeltaBytes * 32:N0} bytes; one full-frame stroke command {fullDeltaBytes:N0} bytes, 32 commands {fullDeltaBytes * 32:N0} bytes");
        _output.WriteLine($"Full-frame delta timings: create {deltaCreateMs:F2} ms; undo {undoMs:F2} ms; redo {redoMs:F2} ms");
    }

    private static LayerRegionCommand CreateCommand(SKBitmap layer, int seed)
    {
        var before = CaptureDocument.SnapshotRegion(layer, new SKRectI(0, 0, 8, 8));
        using var afterLayer = new SKBitmap(new SKImageInfo(8, 8, SKColorType.Bgra8888, SKAlphaType.Premul));
        using (var canvas = new SKCanvas(afterLayer))
            canvas.Clear(seed % 2 == 0 ? SKColors.Blue : SKColors.Green);
        var after = CaptureDocument.SnapshotRegion(afterLayer, new SKRectI(0, 0, 8, 8));
        return new LayerRegionCommand(layer, new SKRectI(0, 0, 8, 8), before, after);
    }

    private static SKBitmap GetBitmap(LayerRegionCommand command, string field) =>
        (SKBitmap)(typeof(LayerRegionCommand).GetField(field, BindingFlags.Instance | BindingFlags.NonPublic)!.GetValue(command)!);

    private static object? GetBitmapOrNull(LayerRegionCommand command, string field) =>
        typeof(LayerRegionCommand).GetField(field, BindingFlags.Instance | BindingFlags.NonPublic)!.GetValue(command);

    private static SKBitmap NewLayer(int width, int height, SKColor color)
    {
        var layer = new SKBitmap(new SKImageInfo(width, height, SKColorType.Bgra8888, SKAlphaType.Premul));
        using var canvas = new SKCanvas(layer);
        canvas.Clear(color);
        return layer;
    }

    private static void SetPixel(SKBitmap bitmap, int x, int y, SKColor color)
    {
        using var canvas = new SKCanvas(bitmap);
        using var paint = new SKPaint { Color = color };
        canvas.DrawPoint(x, y, paint);
    }

    private static byte[] SnapshotBytes(SKBitmap bitmap)
    {
        var bytes = new byte[bitmap.RowBytes * bitmap.Height];
        Marshal.Copy(bitmap.GetPixels(), bytes, 0, bytes.Length);
        return bytes;
    }

    private static void AssertRoundTrip(SKBitmap layer, SKRectI region,
        Action<SKBitmap> mutate, bool expectRaw)
    {
        using var before = CaptureDocument.SnapshotRegion(layer, region);
        var beforeBytes = SnapshotBytes(before);
        mutate(layer);
        using var after = CaptureDocument.SnapshotRegion(layer, region);
        var afterBytes = SnapshotBytes(after);
        var rawBytes = (long)before.ByteCount + after.ByteCount;

        using var roundTrip = new LayerRegionCommand(
            layer, region,
            CopyBitmap(before),
            CopyBitmap(after));

        Assert.True(roundTrip.RetainedBytes <= rawBytes);
        if (expectRaw)
            Assert.Null(GetBitmapOrNull(roundTrip, "_runs"));

        roundTrip.Undo();
        using var restoredBefore = CaptureDocument.SnapshotRegion(layer, region);
        Assert.Equal(beforeBytes, SnapshotBytes(restoredBefore));
        roundTrip.Redo();
        using var restoredAfter = CaptureDocument.SnapshotRegion(layer, region);
        Assert.Equal(afterBytes, SnapshotBytes(restoredAfter));
    }

    private static SKBitmap CopyBitmap(SKBitmap source)
    {
        var copy = new SKBitmap(new SKImageInfo(
            source.Width, source.Height, SKColorType.Bgra8888, SKAlphaType.Premul));
        using var canvas = new SKCanvas(copy);
        canvas.DrawBitmap(source, 0, 0);
        return copy;
    }

    private sealed class TrackingCommand : IUndoableCommand
    {
        private int _disposeCount;

        public int DisposeCount => Volatile.Read(ref _disposeCount);

        public void Undo() { }
        public void Redo() { }

        public void Dispose() => Interlocked.Increment(ref _disposeCount);
    }

    private sealed class BlockingCommand : IUndoableCommand
    {
        private readonly TaskCompletionSource _entered;
        private readonly TaskCompletionSource _release;
        private int _disposeCount;

        public BlockingCommand(TaskCompletionSource entered, TaskCompletionSource release)
        {
            _entered = entered;
            _release = release;
        }

        public int DisposeCount => Volatile.Read(ref _disposeCount);

        public void Undo()
        {
            _entered.TrySetResult();
            _release.Task.GetAwaiter().GetResult();
        }

        public void Redo() { }

        public void Dispose() => Interlocked.Increment(ref _disposeCount);
    }
}
