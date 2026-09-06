using Reshot.Core.Document;
using SkiaSharp;
using System.Runtime.InteropServices;

namespace Reshot.Core.History;

/// <summary>
/// Undo/redo for a raster edit to any layer (paint, effects, absolute mask):
/// stores before/after snapshots of just the affected region (ARCHITECTURE §5).
/// </summary>
public sealed class LayerRegionCommand : IUndoableCommand
{
    private readonly SKBitmap _layer;
    private readonly SKRectI _region;
    private readonly SKBitmap? _before;
    private readonly SKBitmap? _after;
    private readonly List<SnapshotRun>? _runs;
    private readonly int _snapshotOriginX;
    private readonly int _snapshotOriginY;
    private readonly object _gate = new();
    private long _retainedBytes;
    private bool _disposed;

    /// <summary>Payload bytes retained by this command, excluding managed object headers.</summary>
    public long RetainedBytes
    {
        get
        {
            lock (_gate)
                return _retainedBytes;
        }
    }

    /// <summary>
    /// Takes ownership of <paramref name="before"/> and <paramref name="after"/>. The live
    /// <paramref name="layer"/> remains owned by the document and is never disposed here.
    /// </summary>
    public LayerRegionCommand(SKBitmap layer, SKRectI region, SKBitmap before, SKBitmap after)
    {
        _layer = layer;
        _region = region;
        _snapshotOriginX = Math.Clamp(region.Left, 0, layer.Width);
        _snapshotOriginY = Math.Clamp(region.Top, 0, layer.Height);

        if (before.Width == after.Width && before.Height == after.Height
            && TryBuildDelta(before, after, out var runs, out var retainedBytes))
        {
            _runs = runs;
            _retainedBytes = retainedBytes;
            before.Dispose();
            after.Dispose();
        }
        else
        {
            _before = before;
            _after = after;
            _retainedBytes = (long)before.ByteCount + after.ByteCount;
        }
    }

    public void Undo()
    {
        lock (_gate)
        {
            ObjectDisposedException.ThrowIf(_disposed, this);
            if (_runs is not null)
                ApplyRuns(useBefore: true);
            else
                CaptureDocument.RestoreRegion(_layer, _region, _before!);
        }
    }

    public void Redo()
    {
        lock (_gate)
        {
            ObjectDisposedException.ThrowIf(_disposed, this);
            if (_runs is not null)
                ApplyRuns(useBefore: false);
            else
                CaptureDocument.RestoreRegion(_layer, _region, _after!);
        }
    }

    public void Dispose()
    {
        lock (_gate)
        {
            if (_disposed)
                return;
            _disposed = true;
            _before?.Dispose();
            _after?.Dispose();
            _runs?.Clear();
            _retainedBytes = 0;
        }
    }

    private void ApplyRuns(bool useBefore)
    {
        var pixels = _layer.GetPixels();
        foreach (var run in _runs!)
        {
            var x = _snapshotOriginX + run.X;
            var y = _snapshotOriginY + run.Y;
            var offset = checked(y * _layer.RowBytes + x * 4);
            Marshal.Copy(useBefore ? run.Before : run.After, 0,
                IntPtr.Add(pixels, offset), run.Before.Length);
        }
    }

    private static bool TryBuildDelta(SKBitmap before, SKBitmap after,
        out List<SnapshotRun> runs, out long retainedBytes)
    {
        runs = new List<SnapshotRun>();
        retainedBytes = 0;
        var width = before.Width;
        var height = before.Height;
        var beforeRow = new byte[width * 4];
        var afterRow = new byte[width * 4];
        var maxPayload = (long)before.ByteCount + after.ByteCount;
        var beforePixels = before.GetPixels();
        var afterPixels = after.GetPixels();

        for (var y = 0; y < height; y++)
        {
            Marshal.Copy(IntPtr.Add(beforePixels, y * before.RowBytes), beforeRow, 0, beforeRow.Length);
            Marshal.Copy(IntPtr.Add(afterPixels, y * after.RowBytes), afterRow, 0, afterRow.Length);

            var x = 0;
            while (x < width)
            {
                while (x < width && SamePixel(beforeRow, afterRow, x))
                    x++;
                if (x == width)
                    break;

                var start = x;
                while (x < width && !SamePixel(beforeRow, afterRow, x))
                    x++;
                var byteCount = (x - start) * 4;
                var beforeBytes = new byte[byteCount];
                var afterBytes = new byte[byteCount];
                Buffer.BlockCopy(beforeRow, start * 4, beforeBytes, 0, byteCount);
                Buffer.BlockCopy(afterRow, start * 4, afterBytes, 0, byteCount);

                // Include a small per-run allowance so a highly fragmented delta falls back to
                // the raw bitmaps instead of retaining more payload plus object overhead.
                retainedBytes += byteCount * 2L + 16;
                if (retainedBytes > maxPayload)
                {
                    runs.Clear();
                    retainedBytes = 0;
                    return false;
                }

                runs.Add(new SnapshotRun(start, y, beforeBytes, afterBytes));
            }
        }

        return true;
    }

    private static bool SamePixel(byte[] before, byte[] after, int x)
    {
        var i = x * 4;
        return before[i] == after[i]
            && before[i + 1] == after[i + 1]
            && before[i + 2] == after[i + 2]
            && before[i + 3] == after[i + 3];
    }

    private sealed record SnapshotRun(int X, int Y, byte[] Before, byte[] After);
}
