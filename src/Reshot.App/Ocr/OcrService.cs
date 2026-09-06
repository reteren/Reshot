using System.Runtime.InteropServices;
using System.Runtime.InteropServices.WindowsRuntime;
using Reshot.Core.Diagnostics;
using SkiaSharp;
using Windows.Globalization;
using Windows.Graphics.Imaging;
using Windows.Media.Ocr;

namespace Reshot.App.Ocr;

/// <summary>One recognised word with its box in canvas (physical-pixel) coordinates.</summary>
public sealed class OcrWord
{
    public required string Text { get; init; }
    public required SKRect Rect { get; init; }
    public required int Line { get; init; }
}

/// <summary>Recognised words in reading order, plus how many lines they span.</summary>
public sealed class OcrResultModel
{
    public required IReadOnlyList<OcrWord> Words { get; init; }
    public required int LineCount { get; init; }
    public bool IsEmpty => Words.Count == 0;

    public static readonly OcrResultModel Empty =
        new() { Words = Array.Empty<OcrWord>(), LineCount = 0 };
}

/// <summary>
/// Text recognition over a region of the frozen frame, using the OS OCR engine
/// (Windows.Media.Ocr), offline, no models to ship. Windows OCR is one language per
/// engine and a mismatched engine substitutes look-alike glyphs (a RU engine turns
/// Latin "un" into Cyrillic "ип"). The "auto" mode runs both the RU and EN engines and
/// merges per line: the RU pass reliably reports a line's real script, so Latin-dominant
/// lines are taken from EN (clean Latin) and Cyrillic/mixed lines from RU.
/// </summary>
public static class OcrService
{
    public static bool IsAvailable => OcrEngine.AvailableRecognizerLanguages.Count > 0;

    public static IReadOnlyList<string> AvailableLanguageTags() =>
        OcrEngine.AvailableRecognizerLanguages.Select(l => l.LanguageTag).ToList();

    public static bool SupportsLanguage(string tag)
    {
        try { return OcrEngine.IsLanguageSupported(new Language(tag)); }
        catch { return false; }
    }

    private static OcrEngine? CreateEngine(string preferredTag)
    {
        try
        {
            var lang = new Language(preferredTag);
            if (OcrEngine.IsLanguageSupported(lang))
            {
                var e = OcrEngine.TryCreateFromLanguage(lang);
                if (e is not null)
                    return e;
            }
        }
        catch { /* fall through */ }

        return OcrEngine.TryCreateFromUserProfileLanguages()
            ?? (OcrEngine.AvailableRecognizerLanguages.Count > 0
                ? OcrEngine.TryCreateFromLanguage(OcrEngine.AvailableRecognizerLanguages[0])
                : null);
    }

    /// <summary>
    /// Recognises text in <paramref name="region"/> of the frame's BGRA buffer.
    /// <paramref name="mode"/> is "auto" (RU+EN merge), "ru", or "en".
    /// </summary>
    public static async Task<OcrResultModel> RecognizeAsync(
        byte[] frameBgra, int frameStride, SKRectI region, string mode)
    {
        if (region.Width <= 0 || region.Height <= 0 || !IsAvailable)
            return OcrResultModel.Empty;

        // Crop + upscale small selections once; both engines share the bitmap.
        int scale = Math.Max(region.Width, region.Height) < 1200 ? 2 : 1;
        while (scale > 1 && (region.Width * scale > OcrEngine.MaxImageDimension ||
                             region.Height * scale > OcrEngine.MaxImageDimension))
            scale--;

        SoftwareBitmap software;
        if (scale == 1)
        {
            var cropBytes = new byte[region.Width * region.Height * 4];
            int rowBytes = region.Width * 4;
            for (int y = 0; y < region.Height; y++)
            {
                int srcOffset = (region.Top + y) * frameStride + region.Left * 4;
                Buffer.BlockCopy(frameBgra, srcOffset, cropBytes, y * rowBytes, rowBytes);
            }
            software = SoftwareBitmap.CreateCopyFromBuffer(
                cropBytes.AsBuffer(), BitmapPixelFormat.Bgra8, region.Width, region.Height, BitmapAlphaMode.Ignore);
        }
        else
        {
            using var crop = CropRegion(frameBgra, frameStride, region);
            var resized = crop.Resize(new SKImageInfo(region.Width * scale, region.Height * scale), SKFilterQuality.High);
            try
            {
                var bmp = resized ?? crop;
                software = SoftwareBitmap.CreateCopyFromBuffer(
                    bmp.Bytes.AsBuffer(), BitmapPixelFormat.Bgra8, bmp.Width, bmp.Height, BitmapAlphaMode.Ignore);
            }
            finally
            {
                resized?.Dispose();
            }
        }

        OcrResultModel result;
        if (mode == "auto")
        {
            var ru = await RunEngineAsync("ru", software, region, scale);
            var en = await RunEngineAsync("en", software, region, scale);
            result = Merge(ru, en);
        }
        else
        {
            var lines = await RunEngineAsync(mode, software, region, scale);
            result = Flatten(lines);
        }

        Log.Info($"OCR [{mode}]: {result.Words.Count} words / {result.LineCount} lines in {region.Width}x{region.Height} (x{scale}).");
        return result;
    }

    // ---- one engine ------------------------------------------------------------

    private sealed class Line
    {
        public required List<OcrWord> Words { get; init; }
        public float Top { get; init; }
        public float Bottom { get; init; }
        public int Lat { get; init; }
        public int Cyr { get; init; }
    }

    private static async Task<List<Line>> RunEngineAsync(string tag, SoftwareBitmap sw, SKRectI region, int scale)
    {
        var lines = new List<Line>();
        var engine = CreateEngine(tag);
        if (engine is null)
            return lines;

        var res = await engine.RecognizeAsync(sw);
        int li = 0;
        foreach (var line in res.Lines)
        {
            var words = new List<OcrWord>();
            foreach (var word in line.Words)
            {
                var r = word.BoundingRect;
                var rect = new SKRect(
                    region.Left + (float)(r.X / scale),
                    region.Top + (float)(r.Y / scale),
                    region.Left + (float)((r.X + r.Width) / scale),
                    region.Top + (float)((r.Y + r.Height) / scale));
                words.Add(new OcrWord { Text = word.Text, Rect = rect, Line = li });
            }
            if (words.Count > 0)
            {
                lines.Add(new Line
                {
                    Words = words,
                    Top = words.Min(w => w.Rect.Top),
                    Bottom = words.Max(w => w.Rect.Bottom),
                    Lat = CountLatin(line.Text),
                    Cyr = CountCyrillic(line.Text),
                });
            }
            li++;
        }
        return lines;
    }

    // ---- merge -----------------------------------------------------------------

    private static OcrResultModel Merge(List<Line> ru, List<Line> en)
    {
        if (ru.Count == 0) return Flatten(en);
        if (en.Count == 0) return Flatten(ru);

        var words = new List<OcrWord>();
        int lineIdx = 0;
        foreach (var r in ru)
        {
            // The RU pass reports the true script. Cyrillic/mixed lines are best from RU
            // (it also reads embedded Latin fine); Latin-dominant lines are best from EN
            // (RU would swap Latin letters for Cyrillic look-alikes).
            List<OcrWord> chosen = r.Words;
            if (r.Lat > r.Cyr)
            {
                var match = BestOverlap(r, en);
                if (match is not null)
                    chosen = match.Words;
            }

            foreach (var w in chosen)
                words.Add(new OcrWord { Text = w.Text, Rect = w.Rect, Line = lineIdx });
            lineIdx++;
        }
        return new OcrResultModel { Words = words, LineCount = lineIdx };
    }

    private static Line? BestOverlap(Line target, List<Line> candidates)
    {
        Line? best = null;
        float bestOverlap = 0;
        foreach (var c in candidates)
        {
            float overlap = Math.Min(target.Bottom, c.Bottom) - Math.Max(target.Top, c.Top);
            if (overlap > bestOverlap)
            {
                bestOverlap = overlap;
                best = c;
            }
        }
        // Require a real vertical overlap so we don't grab an unrelated line.
        float minH = 0.4f * (target.Bottom - target.Top);
        return bestOverlap >= minH ? best : null;
    }

    private static OcrResultModel Flatten(List<Line> lines)
    {
        var words = new List<OcrWord>();
        for (int i = 0; i < lines.Count; i++)
            foreach (var w in lines[i].Words)
                words.Add(new OcrWord { Text = w.Text, Rect = w.Rect, Line = i });
        return new OcrResultModel { Words = words, LineCount = lines.Count };
    }

    private static int CountLatin(string s) => s.Count(c => c is >= 'A' and <= 'Z' or >= 'a' and <= 'z');
    private static int CountCyrillic(string s) => s.Count(c => c is >= 'Ѐ' and <= 'ӿ');

    // ---- pixel crop ------------------------------------------------------------

    private static SKBitmap CropRegion(byte[] bgra, int stride, SKRectI region)
    {
        var info = new SKImageInfo(region.Width, region.Height, SKColorType.Bgra8888, SKAlphaType.Unpremul);
        var bmp = new SKBitmap(info);
        var dst = bmp.GetPixels();
        int rowBytes = region.Width * 4;
        for (int y = 0; y < region.Height; y++)
        {
            int srcOffset = (region.Top + y) * stride + region.Left * 4;
            Marshal.Copy(bgra, srcOffset, IntPtr.Add(dst, y * rowBytes), rowBytes);
        }
        return bmp;
    }
}
