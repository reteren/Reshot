using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.Linq;
using System.Security.Cryptography;
using System.Windows;
using Reshot.Capture;
using Reshot.Core.Document;
using Reshot.Core.Settings;
using Reshot.Core.Tools;
using SkiaSharp;
using SkiaSharp.Views.Desktop;

namespace Reshot.App.Overlay;

/// <summary>
/// Harness result containing deterministic pixel hash, render metrics, and buffer sizes.
/// </summary>
public sealed class HarnessSceneResult
{
    public required string SceneName { get; init; }
    public required string Sha256Hex { get; init; }
    public required int CanvasWidth { get; init; }
    public required int CanvasHeight { get; init; }
    public required int BufferBytesPerRepaint { get; init; }
    public required double ElapsedMs { get; init; }
    public required byte[] FullFramePixelsBgra { get; init; }

    public int DiffPixelsAgainst(HarnessSceneResult other)
    {
        if (FullFramePixelsBgra.Length != other.FullFramePixelsBgra.Length)
            throw new ArgumentException($"Buffer lengths differ: {FullFramePixelsBgra.Length} vs {other.FullFramePixelsBgra.Length}");

        int diffs = 0;
        for (int i = 0; i < FullFramePixelsBgra.Length; i += 4)
        {
            if (FullFramePixelsBgra[i] != other.FullFramePixelsBgra[i] ||
                FullFramePixelsBgra[i + 1] != other.FullFramePixelsBgra[i + 1] ||
                FullFramePixelsBgra[i + 2] != other.FullFramePixelsBgra[i + 2] ||
                FullFramePixelsBgra[i + 3] != other.FullFramePixelsBgra[i + 3])
            {
                diffs++;
            }
        }
        return diffs;
    }
}

/// <summary>
/// Comparison result evaluating baseline, production fix (7595ad3), unfixed (pre-fix t29),
/// and exact bounds (unioned but unsnapped) to verify regression detection and snap costs.
/// </summary>
public sealed class OutsideCursorTestResult
{
    public required string SceneName { get; init; }
    public required double DpiScale { get; init; }
    public required float BrushThickness { get; init; }
    public required int CursorX { get; init; }
    public required int CursorY { get; init; }
    public required HarnessSceneResult Baseline { get; init; }
    public required HarnessSceneResult Production { get; init; }
    public required HarnessSceneResult Unfixed { get; init; }
    public required HarnessSceneResult ExactNoSnap { get; init; }

    public bool ProductionMatchesBaseline => Baseline.Sha256Hex == Production.Sha256Hex;
    public int ProductionDiffPixels => Baseline.DiffPixelsAgainst(Production);

    public bool UnfixedMatchesBaseline => Baseline.Sha256Hex == Unfixed.Sha256Hex;
    public int UnfixedDiffPixels => Baseline.DiffPixelsAgainst(Unfixed);

    public bool SnapMatchesExact => Production.Sha256Hex == ExactNoSnap.Sha256Hex;
    public int SnapDiffPixelsAgainstExact => ExactNoSnap.DiffPixelsAgainst(Production);

    public int ExactBytes => ExactNoSnap.BufferBytesPerRepaint;
    public int SnappedBytes => Production.BufferBytesPerRepaint;
    public int UnfixedBytes => Unfixed.BufferBytesPerRepaint;
    public int BaselineBytes => Baseline.BufferBytesPerRepaint;

    public int SnapOverheadBytes => SnappedBytes - ExactBytes;
    public double SnapOverheadPercent => ExactBytes == 0 ? 0 : ((double)SnapOverheadBytes / ExactBytes) * 100.0;
}

/// <summary>
/// Audit metrics for the pristine capture state (Job 2).
/// </summary>
public sealed class PristineCaptureAuditResult
{
    public required double DpiScale { get; init; }
    public required bool ShouldRenderDrawSurface { get; init; }
    public required Visibility DrawSurfaceVisibility { get; init; }
    public required int DrawSurfacePixelWidth { get; init; }
    public required int DrawSurfacePixelHeight { get; init; }
    public required int BufferBytesPerRepaint { get; init; }
    public required bool IsZeroCost { get; init; }
}

public partial class OverlayWindow
{
    public void InitializeHeadlessForHarness(double dpiScaleX = 1.0, double dpiScaleY = 1.0)
    {
        _dpiScaleX = dpiScaleX;
        _dpiScaleY = dpiScaleY;
        _document = new CaptureDocument(_frame.Width, _frame.Height);
        EnsureBaseBitmap();
    }

    public void ExecuteSingleSelectionScript(int x, int y, int w, int h)
    {
        _committed.Clear();
        _selection = new Selection
        {
            Kind = ShapeKind.Rectangle,
            Bounds = new Rect(x / _dpiScaleX, y / _dpiScaleY, w / _dpiScaleX, h / _dpiScaleY)
        };

        // Brush stroke
        _tool = ToolMode.Draw;
        _drawSub = DrawSubTool.Brush;
        _brush.Color = new SKColor(0xFF, 0x3B, 0x30);
        _brush.Thickness = 6f;
        _brush.Opacity = 1f;

        BeginBrush(new Point((x + 50) / _dpiScaleX, (y + 50) / _dpiScaleY));
        _cursorPhysical = ToSkPhysical(new Point((x + 150) / _dpiScaleX, (y + 100) / _dpiScaleY));
        _stroke?.Extend(_cursorPhysical);
        _cursorPhysical = ToSkPhysical(new Point((x + 250) / _dpiScaleX, (y + 120) / _dpiScaleY));
        _stroke?.Extend(_cursorPhysical);
        EndBrush();

        // Rectangle shape
        _drawSub = DrawSubTool.Square;
        _brush.Color = new SKColor(0x00, 0x7A, 0xFF);
        _brush.Thickness = 4f;
        BeginShape(new Point((x + 100) / _dpiScaleX, (y + 80) / _dpiScaleY));
        UpdateShape(new SKPoint(x + 300, y + 200));
        EndShape();

        // Text
        _drawSub = DrawSubTool.Text;
        _brush.Color = new SKColor(0x34, 0xC7, 0x59);
        _brush.Thickness = 6f;
        BeginText(new Point((x + 80) / _dpiScaleX, (y + 220) / _dpiScaleY));
        if (_textEditing is not null)
            _textEditing.Text = "Reshot Single Selection";
        CommitText();

        // Blur effect
        _drawSub = DrawSubTool.Blur;
        _brush.Opacity = 0.8f;
        BeginEffect(new Point((x + 120) / _dpiScaleX, (y + 100) / _dpiScaleY));
        _effectRectMode = true;
        _effectRect = SKRect.Create(x + 120, y + 100, 150, 80);
        EndEffect();

        // Highlighter
        _drawSub = DrawSubTool.Brush;
        _brush.Color = new SKColor(0xFF, 0xCC, 0x00);
        _brush.Thickness = 24f;
        _brush.Opacity = 0.4f;
        BeginBrush(new Point((x + 60) / _dpiScaleX, (y + 90) / _dpiScaleY));
        _cursorPhysical = ToSkPhysical(new Point((x + 320) / _dpiScaleX, (y + 180) / _dpiScaleY));
        _stroke?.Extend(_cursorPhysical);
        EndBrush();

        // In-progress stroke & cursor (active drawing state)
        _brush.Color = new SKColor(0xFF, 0x95, 0x00);
        _brush.Thickness = 8f;
        _brush.Opacity = 1f;
        BeginBrush(new Point((x + 70) / _dpiScaleX, (y + 240) / _dpiScaleY));
        _cursorPhysical = ToSkPhysical(new Point((x + 200) / _dpiScaleX, (y + 270) / _dpiScaleY));
        _stroke?.Extend(_cursorPhysical);
        _brushCursorVisible = true;
    }

    public void ExecuteHarnessScript()
    {
        // 1. Primary monitor selection at (2320, 200, 800, 600)
        var selPrimary = new Selection
        {
            Kind = ShapeKind.Rectangle,
            Bounds = new Rect(2320 / _dpiScaleX, 200 / _dpiScaleY, 800 / _dpiScaleX, 600 / _dpiScaleY)
        };
        _committed.Clear();
        _selection = selPrimary;

        // 2. Brush stroke (Red, thickness 6)
        _tool = ToolMode.Draw;
        _drawSub = DrawSubTool.Brush;
        _brush.Color = new SKColor(0xFF, 0x3B, 0x30);
        _brush.Thickness = 6f;
        _brush.Opacity = 1f;

        BeginBrush(new Point(2350 / _dpiScaleX, 250 / _dpiScaleY));
        _cursorPhysical = ToSkPhysical(new Point(2450 / _dpiScaleX, 300 / _dpiScaleY));
        _stroke?.Extend(_cursorPhysical);
        _cursorPhysical = ToSkPhysical(new Point(2550 / _dpiScaleX, 280 / _dpiScaleY));
        _stroke?.Extend(_cursorPhysical);
        _cursorPhysical = ToSkPhysical(new Point(2600 / _dpiScaleX, 350 / _dpiScaleY));
        _stroke?.Extend(_cursorPhysical);
        EndBrush();

        // 3. Rectangle shape (Blue, thickness 4)
        _drawSub = DrawSubTool.Square;
        _brush.Color = new SKColor(0x00, 0x7A, 0xFF);
        _brush.Thickness = 4f;

        BeginShape(new Point(2400 / _dpiScaleX, 320 / _dpiScaleY));
        UpdateShape(new SKPoint(2650, 480));
        EndShape();

        // 4. Text label ("Reshot Test", Green, size 30)
        _drawSub = DrawSubTool.Text;
        _brush.Color = new SKColor(0x34, 0xC7, 0x59);
        _brush.Thickness = 6f;
        BeginText(new Point(2420 / _dpiScaleX, 520 / _dpiScaleY));
        if (_textEditing is not null)
            _textEditing.Text = "Reshot Test";
        CommitText();

        // 5. Blur effect (opacity 0.8)
        _drawSub = DrawSubTool.Blur;
        _brush.Opacity = 0.8f;
        BeginEffect(new Point(2450 / _dpiScaleX, 350 / _dpiScaleY));
        _effectRectMode = true;
        _effectRect = SKRect.Create(2450, 350, 200, 120);
        EndEffect();

        // 6. Pixelize effect (opacity 0.5)
        _drawSub = DrawSubTool.Pixelize;
        _brush.Opacity = 0.5f;
        BeginEffect(new Point(2680 / _dpiScaleX, 350 / _dpiScaleY));
        _effectRectMode = true;
        _effectRect = SKRect.Create(2680, 350, 150, 150);
        EndEffect();

        // 7. Highlighter (Yellow, thickness 24, opacity 0.4) across shapes/text/effects
        _drawSub = DrawSubTool.Brush;
        _brush.Color = new SKColor(0xFF, 0xCC, 0x00);
        _brush.Thickness = 24f;
        _brush.Opacity = 0.4f;

        BeginBrush(new Point(2380 / _dpiScaleX, 330 / _dpiScaleY));
        _cursorPhysical = ToSkPhysical(new Point(2500 / _dpiScaleX, 400 / _dpiScaleY));
        _stroke?.Extend(_cursorPhysical);
        _cursorPhysical = ToSkPhysical(new Point(2700 / _dpiScaleX, 450 / _dpiScaleY));
        _stroke?.Extend(_cursorPhysical);
        _cursorPhysical = ToSkPhysical(new Point(2800 / _dpiScaleX, 530 / _dpiScaleY));
        _stroke?.Extend(_cursorPhysical);
        EndBrush();

        // 8. Secondary monitor selection and drawing
        var selSecondary = new Selection
        {
            Kind = ShapeKind.Rectangle,
            Bounds = new Rect(200 / _dpiScaleX, 400 / _dpiScaleY, 600 / _dpiScaleX, 400 / _dpiScaleY)
        };
        _committed.Add(_selection!);
        _selection = selSecondary;

        _drawSub = DrawSubTool.Brush;
        _brush.Color = new SKColor(0xAF, 0x52, 0xDE); // Purple
        _brush.Thickness = 8f;
        _brush.Opacity = 1f;

        BeginBrush(new Point(250 / _dpiScaleX, 450 / _dpiScaleY));
        _cursorPhysical = ToSkPhysical(new Point(450 / _dpiScaleX, 650 / _dpiScaleY));
        _stroke?.Extend(_cursorPhysical);
        EndBrush();

        // 9. Active live in-progress stroke & cursor (the drawing state)
        _brush.Color = new SKColor(0xFF, 0x95, 0x00); // Orange
        _brush.Thickness = 10f;
        _brush.Opacity = 1f;
        BeginBrush(new Point(300 / _dpiScaleX, 500 / _dpiScaleY));
        _cursorPhysical = ToSkPhysical(new Point(400 / _dpiScaleX, 600 / _dpiScaleY));
        _stroke?.Extend(_cursorPhysical);
        _brushCursorVisible = true;
    }

    /// <summary>
    /// Executes a test scene placing the cursor outside the selection with the Draw tool active.
    /// This targets the boundary case where cursor coordinates depart the selection rectangle.
    /// </summary>
    public void ExecuteCursorOutsideSelectionScript(
        int selX, int selY, int selW, int selH,
        int cursorX, int cursorY,
        float brushThickness = 16f,
        bool commitStroke = true)
    {
        _committed.Clear();
        _selection = new Selection
        {
            Kind = ShapeKind.Rectangle,
            Bounds = new Rect(selX / _dpiScaleX, selY / _dpiScaleY, selW / _dpiScaleX, selH / _dpiScaleY)
        };

        _tool = ToolMode.Draw;
        _drawSub = DrawSubTool.Brush;
        _brush.Color = new SKColor(0xFF, 0x3B, 0x30);
        _brush.Thickness = brushThickness;
        _brush.Opacity = 1f;

        if (commitStroke)
        {
            BeginBrush(new Point((selX + 40) / _dpiScaleX, (selY + 40) / _dpiScaleY));
            _cursorPhysical = ToSkPhysical(new Point((selX + 120) / _dpiScaleX, (selY + 80) / _dpiScaleY));
            _stroke?.Extend(_cursorPhysical);
            EndBrush();
        }

        _cursorPhysical = ToSkPhysical(new Point(cursorX / _dpiScaleX, cursorY / _dpiScaleY));
        _brushCursorVisible = true;
    }

    public HarnessSceneResult RenderBaselineDirect(string sceneName)
    {
        var oldX = _drawSurfacePxX;
        var oldY = _drawSurfacePxY;
        _drawSurfacePxX = 0;
        _drawSurfacePxY = 0;

        try
        {
            var sw = Stopwatch.StartNew();
            var fullInfo = new SKImageInfo(_frame.Width, _frame.Height, SKColorType.Bgra8888, SKAlphaType.Premul);
            using var surface = SKSurface.Create(fullInfo);
            OnPaintSurface(this, new SKPaintSurfaceEventArgs(surface, fullInfo));
            sw.Stop();

            var fullPixels = new byte[_frame.Width * _frame.Height * 4];
            var pixmap = surface.PeekPixels();
            System.Runtime.InteropServices.Marshal.Copy(pixmap.GetPixels(), fullPixels, 0, fullPixels.Length);

            var hashBytes = SHA256.HashData(fullPixels);
            var sha256Hex = Convert.ToHexString(hashBytes);

            return new HarnessSceneResult
            {
                SceneName = sceneName,
                Sha256Hex = sha256Hex,
                CanvasWidth = _frame.Width,
                CanvasHeight = _frame.Height,
                BufferBytesPerRepaint = _frame.Width * _frame.Height * 4,
                ElapsedMs = sw.Elapsed.TotalMilliseconds,
                FullFramePixelsBgra = fullPixels,
            };
        }
        finally
        {
            _drawSurfacePxX = oldX;
            _drawSurfacePxY = oldY;
        }
    }

    /// <summary>
    /// Pre-fix t29 logic: padded only the selection bounding box, neglecting pointer departure.
    /// Used as the negative control to prove regression test validity.
    /// </summary>
    public void UpdateDrawSurfaceBoundsUnfixed()
    {
        var sels = AllSelections().ToList();
        if (sels.Count == 0 && _contentBounds.IsEmpty)
        {
            ApplyDrawSurfaceBounds(0, 0, 0, 0);
            return;
        }

        int minX = int.MaxValue, minY = int.MaxValue;
        int maxX = int.MinValue, maxY = int.MinValue;

        foreach (var s in sels)
        {
            var px = BoundsToPixels(s.Bounds);
            minX = Math.Min(minX, px.X);
            minY = Math.Min(minY, px.Y);
            maxX = Math.Max(maxX, px.X + px.Width);
            maxY = Math.Max(maxY, px.Y + px.Height);
        }

        if (!_contentBounds.IsEmpty)
        {
            minX = Math.Min(minX, _contentBounds.Left);
            minY = Math.Min(minY, _contentBounds.Top);
            maxX = Math.Max(maxX, _contentBounds.Right);
            maxY = Math.Max(maxY, _contentBounds.Bottom);
        }

        if (_brushCursorVisible && _tool == ToolMode.Draw)
        {
            var pad = (int)Math.Ceiling(_brush.Thickness / 2f + 4f);
            minX -= pad;
            minY -= pad;
            maxX += pad;
            maxY += pad;
        }

        minX = Math.Clamp(minX, 0, _frame.Width);
        minY = Math.Clamp(minY, 0, _frame.Height);
        maxX = Math.Clamp(maxX, minX, _frame.Width);
        maxY = Math.Clamp(maxY, minY, _frame.Height);

        ApplyDrawSurfaceBounds(minX, minY, maxX - minX, maxY - minY);
    }

    /// <summary>
    /// Exact cursor-union bounds without grid snapping.
    /// Proves that the 64-px snap produces 0 pixel diffs and measures the snap byte overhead.
    /// </summary>
    public void UpdateDrawSurfaceBoundsExact()
    {
        var sels = AllSelections().ToList();
        if (sels.Count == 0 && _contentBounds.IsEmpty)
        {
            ApplyDrawSurfaceBounds(0, 0, 0, 0);
            return;
        }

        int minX = int.MaxValue, minY = int.MaxValue;
        int maxX = int.MinValue, maxY = int.MinValue;

        foreach (var s in sels)
        {
            var px = BoundsToPixels(s.Bounds);
            minX = Math.Min(minX, px.X);
            minY = Math.Min(minY, px.Y);
            maxX = Math.Max(maxX, px.X + px.Width);
            maxY = Math.Max(maxY, px.Y + px.Height);
        }

        if (!_contentBounds.IsEmpty)
        {
            minX = Math.Min(minX, _contentBounds.Left);
            minY = Math.Min(minY, _contentBounds.Top);
            maxX = Math.Max(maxX, _contentBounds.Right);
            maxY = Math.Max(maxY, _contentBounds.Bottom);
        }

        if (_brushCursorVisible && _tool == ToolMode.Draw)
        {
            var pad = (int)Math.Ceiling(_brush.Thickness / 2f + 4f);
            minX = Math.Min(minX, (int)Math.Floor(_cursorPhysical.X) - pad);
            minY = Math.Min(minY, (int)Math.Floor(_cursorPhysical.Y) - pad);
            maxX = Math.Max(maxX, (int)Math.Ceiling(_cursorPhysical.X) + pad);
            maxY = Math.Max(maxY, (int)Math.Ceiling(_cursorPhysical.Y) + pad);
        }

        minX = Math.Clamp(minX, 0, _frame.Width);
        minY = Math.Clamp(minY, 0, _frame.Height);
        maxX = Math.Clamp(maxX, minX, _frame.Width);
        maxY = Math.Clamp(maxY, minY, _frame.Height);

        ApplyDrawSurfaceBounds(minX, minY, maxX - minX, maxY - minY);
    }

    /// <summary>
    /// Outward snap to an arbitrary grid size (e.g. 16, 32, 64, 128) to quantify grid constant trade-offs.
    /// </summary>
    public void UpdateDrawSurfaceBoundsGrid(int grid)
    {
        var sels = AllSelections().ToList();
        if (sels.Count == 0 && _contentBounds.IsEmpty)
        {
            ApplyDrawSurfaceBounds(0, 0, 0, 0);
            return;
        }

        int minX = int.MaxValue, minY = int.MaxValue;
        int maxX = int.MinValue, maxY = int.MinValue;

        foreach (var s in sels)
        {
            var px = BoundsToPixels(s.Bounds);
            minX = Math.Min(minX, px.X);
            minY = Math.Min(minY, px.Y);
            maxX = Math.Max(maxX, px.X + px.Width);
            maxY = Math.Max(maxY, px.Y + px.Height);
        }

        if (!_contentBounds.IsEmpty)
        {
            minX = Math.Min(minX, _contentBounds.Left);
            minY = Math.Min(minY, _contentBounds.Top);
            maxX = Math.Max(maxX, _contentBounds.Right);
            maxY = Math.Max(maxY, _contentBounds.Bottom);
        }

        if (_brushCursorVisible && _tool == ToolMode.Draw)
        {
            var pad = (int)Math.Ceiling(_brush.Thickness / 2f + 4f);
            minX = Math.Min(minX, (int)Math.Floor(_cursorPhysical.X) - pad);
            minY = Math.Min(minY, (int)Math.Floor(_cursorPhysical.Y) - pad);
            maxX = Math.Max(maxX, (int)Math.Ceiling(_cursorPhysical.X) + pad);
            maxY = Math.Max(maxY, (int)Math.Ceiling(_cursorPhysical.Y) + pad);
        }

        if (grid > 1)
        {
            minX = minX / grid * grid;
            minY = minY / grid * grid;
            maxX = (maxX + grid - 1) / grid * grid;
            maxY = (maxY + grid - 1) / grid * grid;
        }

        minX = Math.Clamp(minX, 0, _frame.Width);
        minY = Math.Clamp(minY, 0, _frame.Height);
        maxX = Math.Clamp(maxX, minX, _frame.Width);
        maxY = Math.Clamp(maxY, minY, _frame.Height);

        ApplyDrawSurfaceBounds(minX, minY, maxX - minX, maxY - minY);
    }

    public HarnessSceneResult RenderWithBoundsAction(string sceneName, Action boundsAction)
    {
        boundsAction();

        if (_drawSurfacePxW <= 0 || _drawSurfacePxH <= 0)
        {
            var emptyPixels = new byte[_frame.Width * _frame.Height * 4];
            return new HarnessSceneResult
            {
                SceneName = sceneName,
                Sha256Hex = Convert.ToHexString(SHA256.HashData(emptyPixels)),
                CanvasWidth = 0,
                CanvasHeight = 0,
                BufferBytesPerRepaint = 0,
                ElapsedMs = 0,
                FullFramePixelsBgra = emptyPixels,
            };
        }

        var sw = Stopwatch.StartNew();
        var sizedInfo = new SKImageInfo(_drawSurfacePxW, _drawSurfacePxH, SKColorType.Bgra8888, SKAlphaType.Premul);
        using var sizedSurface = SKSurface.Create(sizedInfo);
        OnPaintSurface(this, new SKPaintSurfaceEventArgs(sizedSurface, sizedInfo));
        sw.Stop();

        var fullInfo = new SKImageInfo(_frame.Width, _frame.Height, SKColorType.Bgra8888, SKAlphaType.Premul);
        using var fullSurface = SKSurface.Create(fullInfo);
        fullSurface.Canvas.Clear(SKColors.Transparent);
        using var snapshot = sizedSurface.Snapshot();
        fullSurface.Canvas.DrawImage(snapshot, _drawSurfacePxX, _drawSurfacePxY);

        var fullPixels = new byte[_frame.Width * _frame.Height * 4];
        var pixmap = fullSurface.PeekPixels();
        System.Runtime.InteropServices.Marshal.Copy(pixmap.GetPixels(), fullPixels, 0, fullPixels.Length);

        var hashBytes = SHA256.HashData(fullPixels);
        var sha256Hex = Convert.ToHexString(hashBytes);

        return new HarnessSceneResult
        {
            SceneName = sceneName,
            Sha256Hex = sha256Hex,
            CanvasWidth = _drawSurfacePxW,
            CanvasHeight = _drawSurfacePxH,
            BufferBytesPerRepaint = _drawSurfacePxW * _drawSurfacePxH * 4,
            ElapsedMs = sw.Elapsed.TotalMilliseconds,
            FullFramePixelsBgra = fullPixels,
        };
    }

    public HarnessSceneResult RenderOptimizedProduction(string sceneName) =>
        RenderWithBoundsAction(sceneName, UpdateDrawSurfaceBounds);

    public HarnessSceneResult RenderUnfixed(string sceneName) =>
        RenderWithBoundsAction(sceneName, UpdateDrawSurfaceBoundsUnfixed);

    public HarnessSceneResult RenderExactNoSnap(string sceneName) =>
        RenderWithBoundsAction(sceneName, UpdateDrawSurfaceBoundsExact);

    public HarnessSceneResult RenderGridSnap(string sceneName, int grid) =>
        RenderWithBoundsAction(sceneName, () => UpdateDrawSurfaceBoundsGrid(grid));

    public bool GetShouldRenderDrawSurface() => ShouldRenderDrawSurface;
    public Visibility GetDrawSurfaceVisibility() => DrawSurface.Visibility;
    public (int X, int Y, int W, int H) GetDrawSurfacePixelBounds() =>
        (_drawSurfacePxX, _drawSurfacePxY, _drawSurfacePxW, _drawSurfacePxH);
}

public static class OverlayPixelHarness
{
    public static CapturedFrame CreateSyntheticFrame(int width = 4480, int height = 1440)
    {
        var pixels = new byte[width * height * 4];
        for (int y = 0; y < height; y++)
        {
            for (int x = 0; x < width; x++)
            {
                int i = (y * width + x) * 4;
                pixels[i + 0] = (byte)((x ^ y) & 0xFF);         // Blue
                pixels[i + 1] = (byte)((x * 3 + y * 7) & 0xFF); // Green
                pixels[i + 2] = (byte)((x * 11 + y * 5) & 0xFF);// Red
                pixels[i + 3] = 255;                            // Alpha
            }
        }

        var monitors = new List<CapturedMonitor>
        {
            new(-1920, 357, 1920, 1080, false),
            new(0, 0, 2560, 1440, true),
        };

        return new CapturedFrame
        {
            PixelsBgra = pixels,
            Width = width,
            Height = height,
            VirtualLeft = -1920,
            VirtualTop = 0,
            Monitors = monitors,
        };
    }

    public static HarnessSceneResult RunBaselineScene(double dpiScaleX = 1.0, double dpiScaleY = 1.0)
    {
        var frame = CreateSyntheticFrame();
        var settings = new AppSettings();
        var window = new OverlayWindow(frame, settings);
        window.InitializeHeadlessForHarness(dpiScaleX, dpiScaleY);
        window.ExecuteHarnessScript();
        return window.RenderBaselineDirect("StandardScriptedScene_Baseline");
    }

    public static HarnessSceneResult RunOptimizedScene(double dpiScaleX = 1.0, double dpiScaleY = 1.0)
    {
        var frame = CreateSyntheticFrame();
        var settings = new AppSettings();
        var window = new OverlayWindow(frame, settings);
        window.InitializeHeadlessForHarness(dpiScaleX, dpiScaleY);
        window.ExecuteHarnessScript();
        return window.RenderOptimizedProduction("StandardScriptedScene_Optimized");
    }

    public static (HarnessSceneResult Baseline, HarnessSceneResult Optimized) RunSingleSelectionComparison(
        int x, int y, int w, int h, double dpiScaleX = 1.0, double dpiScaleY = 1.0)
    {
        var frame1 = CreateSyntheticFrame();
        var window1 = new OverlayWindow(frame1, new AppSettings());
        window1.InitializeHeadlessForHarness(dpiScaleX, dpiScaleY);
        window1.ExecuteSingleSelectionScript(x, y, w, h);
        var baseline = window1.RenderBaselineDirect($"Single_{w}x{h}_Baseline");

        var frame2 = CreateSyntheticFrame();
        var window2 = new OverlayWindow(frame2, new AppSettings());
        window2.InitializeHeadlessForHarness(dpiScaleX, dpiScaleY);
        window2.ExecuteSingleSelectionScript(x, y, w, h);
        var optimized = window2.RenderOptimizedProduction($"Single_{w}x{h}_Optimized");

        return (baseline, optimized);
    }

    public static OutsideCursorTestResult RunOutsideCursorComparison(
        int selX, int selY, int selW, int selH,
        int cursorX, int cursorY,
        float thickness,
        double dpiScale = 1.0,
        string sceneName = "")
    {
        var name = string.IsNullOrEmpty(sceneName)
            ? $"Sel_{selW}x{selH}_Cursor_{cursorX}_{cursorY}_Thick_{thickness}_Dpi_{dpiScale * 100:0}%"
            : sceneName;

        var f1 = CreateSyntheticFrame();
        var w1 = new OverlayWindow(f1, new AppSettings());
        w1.InitializeHeadlessForHarness(dpiScale, dpiScale);
        w1.ExecuteCursorOutsideSelectionScript(selX, selY, selW, selH, cursorX, cursorY, thickness);
        var baseline = w1.RenderBaselineDirect($"{name}_Baseline");

        var f2 = CreateSyntheticFrame();
        var w2 = new OverlayWindow(f2, new AppSettings());
        w2.InitializeHeadlessForHarness(dpiScale, dpiScale);
        w2.ExecuteCursorOutsideSelectionScript(selX, selY, selW, selH, cursorX, cursorY, thickness);
        var production = w2.RenderOptimizedProduction($"{name}_Production");

        var f3 = CreateSyntheticFrame();
        var w3 = new OverlayWindow(f3, new AppSettings());
        w3.InitializeHeadlessForHarness(dpiScale, dpiScale);
        w3.ExecuteCursorOutsideSelectionScript(selX, selY, selW, selH, cursorX, cursorY, thickness);
        var unfixed = w3.RenderUnfixed($"{name}_Unfixed");

        var f4 = CreateSyntheticFrame();
        var w4 = new OverlayWindow(f4, new AppSettings());
        w4.InitializeHeadlessForHarness(dpiScale, dpiScale);
        w4.ExecuteCursorOutsideSelectionScript(selX, selY, selW, selH, cursorX, cursorY, thickness);
        var exact = w4.RenderExactNoSnap($"{name}_ExactNoSnap");

        return new OutsideCursorTestResult
        {
            SceneName = name,
            DpiScale = dpiScale,
            BrushThickness = thickness,
            CursorX = cursorX,
            CursorY = cursorY,
            Baseline = baseline,
            Production = production,
            Unfixed = unfixed,
            ExactNoSnap = exact,
        };
    }

    public static PristineCaptureAuditResult RunPristineCaptureAudit(double dpiScale = 1.0)
    {
        var frame = CreateSyntheticFrame();
        var settings = new AppSettings();
        var window = new OverlayWindow(frame, settings);
        window.InitializeHeadlessForHarness(dpiScale, dpiScale);

        bool shouldRender = window.GetShouldRenderDrawSurface();
        var visibility = window.GetDrawSurfaceVisibility();
        var (pxX, pxY, pxW, pxH) = window.GetDrawSurfacePixelBounds();
        int bufferBytes = (visibility == Visibility.Visible) ? pxW * pxH * 4 : 0;

        return new PristineCaptureAuditResult
        {
            DpiScale = dpiScale,
            ShouldRenderDrawSurface = shouldRender,
            DrawSurfaceVisibility = visibility,
            DrawSurfacePixelWidth = pxW,
            DrawSurfacePixelHeight = pxH,
            BufferBytesPerRepaint = bufferBytes,
            IsZeroCost = !shouldRender && visibility == Visibility.Collapsed && bufferBytes == 0,
        };
    }
}
