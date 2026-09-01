using System.Collections.Concurrent;
using SkiaSharp;

namespace Reshot.Core.Tools;

/// <summary>One installed font family, as offered by the Text tool's family picker.</summary>
/// <param name="Family">Family name as reported by the system font manager.</param>
/// <param name="SupportsCyrillic">
/// False when the family has no Cyrillic glyphs. Skia's DrawText does not substitute another
/// family for missing glyphs, so such a font renders Russian text as tofu boxes; the UI warns
/// instead of silently drawing garbage.
/// </param>
public sealed record FontEntry(string Family, bool SupportsCyrillic);

/// <summary>
/// The installed system fonts, for the Text tool (SPEC 6.4). Pure Skia, so Reshot.Core stays
/// free of WPF and its font stack.
///
/// Building the catalogue opens every installed font file to probe glyph coverage, which is why
/// it is lazy and why <see cref="Warmup"/> exists: the overlay must never pay for it on the UI
/// thread. Resolved typefaces are cached forever and shared between the picker and the renderer.
/// </summary>
public static class FontCatalog
{
    /// <summary>Family used when none is chosen, or the chosen one is not installed.</summary>
    public const string DefaultFamily = "Segoe UI";

    // The two ends of the Cyrillic block. A family carrying both effectively always carries the
    // whole alphabet, and two probes keep the catalogue build cheap.
    private const int CyrillicCapitalA = 0x0410;
    private const int CyrillicSmallYa = 0x044F;

    private static readonly Lazy<IReadOnlyList<FontEntry>> _catalog =
        new(Build, LazyThreadSafetyMode.ExecutionAndPublication);

    // Never disposed and deliberately so: these instances are handed to callers who put them on
    // an SKPaint and dispose that paint every frame. Disposing an SKPaint in SkiaSharp 2.88.9
    // does not dispose the assigned Typeface, so one instance per family is safe to share.
    private static readonly ConcurrentDictionary<string, SKTypeface> _typefaces =
        new(StringComparer.OrdinalIgnoreCase);

    /// <summary>
    /// Every installed family, <see cref="DefaultFamily"/> first and the rest sorted
    /// case-insensitively. Built once, on first access; see <see cref="Warmup"/>.
    /// </summary>
    public static IReadOnlyList<FontEntry> Families => _catalog.Value;

    /// <summary>
    /// The typeface for a family, cached one per family. A blank or not-installed name falls back
    /// to <see cref="DefaultFamily"/> and returns that family's shared instance, so a stale font
    /// name in saved settings measures and draws identically to the default. Never null, never
    /// throws. Do not dispose the result.
    ///
    /// Deliberately independent of <see cref="Families"/>: this runs on the UI thread on every
    /// redraw, and touching the lazy catalogue here would make the first draw block until every
    /// installed font file had been opened and probed.
    /// </summary>
    public static SKTypeface Resolve(string? family)
    {
        var name = family?.Trim();
        return string.IsNullOrEmpty(name) ? Cached(DefaultFamily) : _typefaces.GetOrAdd(name, OpenInstalled);
    }

    /// <summary>
    /// Builds the catalogue on a background thread so the first UI touch is instant. Cheap and
    /// safe to call more than once; the build itself happens exactly once.
    /// </summary>
    public static void Warmup()
    {
        _ = Task.Run(() =>
        {
            try
            {
                _ = Families;
            }
            catch (Exception)
            {
                // Unreachable in practice - Build degrades rather than throwing - but an
                // unobserved task exception must never reach the thread pool.
            }
        });
    }

    /// <summary>Typeface for a name already known to be installed, or for the default family.</summary>
    private static SKTypeface Cached(string family) => _typefaces.GetOrAdd(family, Open);

    /// <summary>
    /// Typeface for a name that may not be installed. Skia answers an unknown family with a
    /// silently substituted face, which would then be cached under the unknown name as a second
    /// handle onto what is really the default font; comparing the family that came back catches
    /// that without the catalogue. The substitute is dropped rather than disposed, because
    /// SkiaSharp hands out one managed instance per native handle and that handle may already be
    /// the cached default.
    /// </summary>
    private static SKTypeface OpenInstalled(string family)
    {
        var typeface = Open(family);
        return string.Equals(typeface.FamilyName, family, StringComparison.OrdinalIgnoreCase)
            ? typeface
            : Cached(DefaultFamily);
    }

    private static SKTypeface Open(string family)
    {
        try
        {
            // On Windows FromFamilyName always returns a face (falling back to the system UI
            // font); other backends can return null, hence the explicit ladder.
            return SKTypeface.FromFamilyName(family)
                ?? SKTypeface.FromFamilyName(DefaultFamily)
                ?? SKTypeface.Default;
        }
        catch (Exception)
        {
            return SKTypeface.Default;
        }
    }

    private static IReadOnlyList<FontEntry> Build()
    {
        try
        {
            var names = new List<string>();
            var seen = new HashSet<string>(StringComparer.OrdinalIgnoreCase);
            foreach (var name in SKFontManager.Default.FontFamilies)
            {
                if (!string.IsNullOrWhiteSpace(name) && seen.Add(name))
                    names.Add(name);
            }

            // Ordinal, not the current culture: the same machine must produce the same order for
            // every user, and a Turkish or Swedish collation would otherwise reorder the list
            // under a build agent running the same assertions.
            names.Sort(StringComparer.OrdinalIgnoreCase);

            // The default leads the dropdown so it opens on the familiar choice rather than on
            // whatever font happens to sort first.
            var index = names.FindIndex(
                n => string.Equals(n, DefaultFamily, StringComparison.OrdinalIgnoreCase));
            if (index > 0)
            {
                var preferred = names[index];
                names.RemoveAt(index);
                names.Insert(0, preferred);
            }

            var entries = new List<FontEntry>(names.Count);
            foreach (var name in names)
                entries.Add(new FontEntry(name, HasCyrillic(name)));

            return entries;
        }
        catch (Exception)
        {
            // A font manager that cannot enumerate must still leave the picker with something to
            // show, and must not poison the Lazy with a cached exception.
            return new[] { new FontEntry(DefaultFamily, HasCyrillic(DefaultFamily)) };
        }
    }

    private static bool HasCyrillic(string family)
    {
        // Goes through Cached, not Resolve: Resolve would re-enter the Lazy that is running this
        // build. Sharing the cache also means each font file is opened once, not once per probe.
        var typeface = Cached(family);
        try
        {
            return typeface.ContainsGlyph(CyrillicCapitalA) && typeface.ContainsGlyph(CyrillicSmallYa);
        }
        catch (Exception)
        {
            return false;
        }
    }
}
