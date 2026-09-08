using System.IO;
using System.Windows;
using System.Windows.Media;
using System.Windows.Media.Imaging;
using Microsoft.Win32;
using Reshot.App.Overlay;
using Reshot.Core;
using Reshot.Core.Diagnostics;
using Reshot.Core.Export;
using Reshot.Core.Settings;
using SkiaSharp;
using Clipboard = System.Windows.Clipboard;
using DataObject = System.Windows.DataObject;
using SaveFileDialog = Microsoft.Win32.SaveFileDialog;

namespace Reshot.App.Export;

/// <summary>
/// Turns a selection region of the frozen frame into clipboard / file output
/// (SPEC §10). Supports arbitrary shapes: pixels outside the shape geometry are
/// made transparent (PNG alpha), including on the clipboard.
/// </summary>
public sealed class CaptureExporter
{
    private readonly AppSettings _settings;

    public CaptureExporter(AppSettings settings) => _settings = settings;

    /// <summary>Plain rectangular crop of the frozen frame (physical pixels).</summary>
    public static BitmapSource Crop(BitmapSource fullFrame, Int32Rect region)
    {
        var cropped = new CroppedBitmap(fullFrame, region);
        cropped.Freeze();
        return cropped;
    }

    /// <summary>
    /// Crops <paramref name="region"/>, draws the optional <paramref name="overlayFull"/>
    /// (e.g. the paint layer) over the frame, and keeps only what's inside
    /// <paramref name="localMask"/> (geometry in crop-local physical pixels);
    /// everything outside becomes transparent.
    /// </summary>
    public static BitmapSource CropMasked(
        BitmapSource fullFrame, Int32Rect region, Geometry? localMask, BitmapSource? overlayFull = null)
    {
        var crop = Crop(fullFrame, region);
        if (localMask is null && overlayFull is null)
            return crop;

        var overlayCrop = overlayFull is not null ? Crop(overlayFull, region) : null;
        var area = new Rect(0, 0, region.Width, region.Height);

        var visual = new DrawingVisual();
        using (var dc = visual.RenderOpen())
        {
            if (localMask is not null)
                dc.PushClip(localMask);
            dc.DrawImage(crop, area);
            if (overlayCrop is not null)
                dc.DrawImage(overlayCrop, area);
            if (localMask is not null)
                dc.Pop();
        }

        var rtb = new RenderTargetBitmap(region.Width, region.Height, 96, 96, PixelFormats.Pbgra32);
        rtb.Render(visual);
        rtb.Freeze();
        return rtb;
    }

    /// <summary>
    /// Copies on a dedicated STA so PNG/DIB materialisation and OLE clipboard ownership do not
    /// block the overlay dispatcher. The source image is frozen by the export path before this
    /// method is called, so it is safe to consume from the worker thread.
    /// </summary>
    public Task<bool> CopyToClipboardAsync(BitmapSource image)
    {
        var result = new TaskCompletionSource<bool>(TaskCreationOptions.RunContinuationsAsynchronously);
        var thread = new Thread(() =>
        {
            try
            {
                result.SetResult(CopyToClipboardOnSta(image));
            }
            catch (Exception ex)
            {
                Log.Error("Export: clipboard worker failed", ex);
                result.SetResult(false);
            }
        })
        {
            IsBackground = true,
            Name = "Reshot clipboard export",
        };
        thread.SetApartmentState(ApartmentState.STA);
        thread.Start();
        return result.Task;
    }

    private bool CopyToClipboardOnSta(BitmapSource image)
    {
        // WPF itself retries ten times at 100 ms intervals before reporting a busy clipboard.
        // An outer retry would stack another full WPF wait and make a transient contention
        // last several seconds without improving the eventual clipboard ownership.
        try
        {
            Clipboard.SetDataObject(BuildClipboardData(image), copy: true);
            Log.Info("Export: copied selection to clipboard.");
            return true;
        }
        catch (Exception ex)
        {
            Log.Warn($"Export: clipboard busy or unavailable: {ex.Message}");
            return false;
        }
    }

    /// <summary>
    /// Packs the image both as a PNG stream (apps like Telegram/Discord/Paint.NET
    /// read this and keep transparency) and as a plain bitmap (DIB) fallback.
    /// </summary>
    private static DataObject BuildClipboardData(BitmapSource image)
    {
        var data = new DataObject();

        var estimatedCapacity = Math.Max(8192, Math.Min(image.PixelWidth * image.PixelHeight, 4 * 1024 * 1024));
        var ms = new MemoryStream(estimatedCapacity);
        var encoder = new PngBitmapEncoder();
        encoder.Frames.Add(BitmapFrame.Create(image));
        encoder.Save(ms);
        ms.Position = 0;
        data.SetData("PNG", ms);

        // DIB fallback (loses alpha) for apps that only understand bitmaps.
        data.SetImage(image);
        return data;
    }

    /// <summary>Saves to the configured screenshots folder with the templated name.</summary>
    public string Save(BitmapSource image)
    {
        var dir = ResolveScreenshotDir();
        Directory.CreateDirectory(dir);
        var fileName = FilenameBuilder.Build(_settings.Filename.Template, _settings.Format.Image, DateTime.Now);
        var path = Path.Combine(dir, fileName);
        EncodeToFile(image, path);
        Log.Info($"Export: saved {path}");
        return path;
    }

    /// <summary>Prompts for a destination; returns the saved path, or null if cancelled.</summary>
    public string? SaveAs(BitmapSource image, Window? owner = null)
    {
        var format = _settings.Format.Image.ToLowerInvariant();
        var dialog = new SaveFileDialog
        {
            Title = "Save screenshot",
            InitialDirectory = ResolveScreenshotDir(),
            FileName = FilenameBuilder.Build(_settings.Filename.Template, format, DateTime.Now),
            Filter = "PNG image|*.png|JPEG image|*.jpg|WebP image|*.webp",
            FilterIndex = format switch { "jpg" or "jpeg" => 2, "webp" => 3, _ => 1 },
            DefaultExt = "." + format,
            AddExtension = true,
        };

        // Owned by the overlay so the dialog surfaces above the topmost window.
        var ok = owner is not null ? dialog.ShowDialog(owner) : dialog.ShowDialog();
        if (ok != true)
            return null;

        EncodeToFile(image, dialog.FileName);
        Log.Info($"Export: saved as {dialog.FileName}");
        return dialog.FileName;
    }

    private string ResolveScreenshotDir()
    {
        var dir = _settings.Paths.Screenshots;
        return string.IsNullOrWhiteSpace(dir) ? ReshotPaths.DefaultScreenshotsDir : dir;
    }

    private void EncodeToFile(BitmapSource image, string path)
    {
        var quality = Math.Clamp(_settings.Format.Quality, 1, 100);
        var ext = Path.GetExtension(path).ToLowerInvariant();

        // WPF has no WebP encoder, route it through Skia (lossy, keeps alpha).
        if (ext == ".webp")
        {
            using var skBitmap = SkiaInterop.ToSkBitmap(image);
            using var skImage = SKImage.FromBitmap(skBitmap);
            using var data = skImage.Encode(SKEncodedImageFormat.Webp, quality);
            using var webpStream = File.Create(path);
            data.SaveTo(webpStream);
            return;
        }

        BitmapEncoder encoder = ext switch
        {
            ".jpg" or ".jpeg" => new JpegBitmapEncoder { QualityLevel = quality },
            _ => new PngBitmapEncoder(),
        };
        encoder.Frames.Add(BitmapFrame.Create(image));
        using var fs = File.Create(path);
        encoder.Save(fs);
    }
}
