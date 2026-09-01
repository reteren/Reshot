using Reshot.Core.Tools;
using SkiaSharp;
using Xunit;

namespace Reshot.Core.Tests;

// The overlay fills its font dropdown from FontCatalog.Families and calls Resolve() once per
// redraw, so the catalogue has to be well-formed, cached, and — above all — outlive the throwaway
// SKPaint objects the renderer disposes every frame.
public class FontCatalogTests
{
    // A name no font vendor will ever ship; exercises the unknown-family fallback.
    private const string MissingFamily = "NoSuchFontFamily_ZZZ";

    [Fact]
    public void Families_is_populated_and_free_of_blanks_and_duplicates()
    {
        var families = FontCatalog.Families;

        Assert.NotEmpty(families);
        Assert.All(families, f => Assert.False(string.IsNullOrWhiteSpace(f.Family), "blank family name"));

        // A duplicate would show up twice in the dropdown and make SelectedValue ambiguous.
        var seen = new HashSet<string>(StringComparer.OrdinalIgnoreCase);
        foreach (var f in families)
            Assert.True(seen.Add(f.Family), $"duplicate family '{f.Family}'");
    }

    [Fact]
    public void Families_after_the_default_are_sorted_case_insensitively()
    {
        var rest = FontCatalog.Families.Skip(1).Select(f => f.Family).ToList();

        // Ordinal and invariant-culture case-insensitive ordering disagree on names starting with
        // punctuation or digits, and which comparer the catalogue used is an implementation detail.
        // Either one is "sorted" as far as someone scrolling the dropdown is concerned.
        Assert.True(
            IsSorted(rest, StringComparer.OrdinalIgnoreCase) ||
            IsSorted(rest, StringComparer.InvariantCultureIgnoreCase),
            $"font list is not sorted: {FirstDisorder(rest)}");
    }

    [Fact]
    public void Default_family_is_listed_first_when_installed()
    {
        var families = FontCatalog.Families;
        var hasDefault = families.Any(f =>
            string.Equals(f.Family, FontCatalog.DefaultFamily, StringComparison.OrdinalIgnoreCase));

        // Windows always ships Segoe UI, but assert against what is really there rather than
        // against that assumption — a machine without it has nothing to pin the ordering to.
        if (!hasDefault)
            return;

        Assert.True(
            string.Equals(families[0].Family, FontCatalog.DefaultFamily, StringComparison.OrdinalIgnoreCase),
            $"expected '{FontCatalog.DefaultFamily}' first, but the list starts with '{families[0].Family}'");
    }

    [Theory]
    [InlineData(null)]
    [InlineData("")]
    [InlineData("   ")]
    [InlineData("\t")]
    [InlineData(MissingFamily)]
    public void Resolve_falls_back_to_the_default_typeface(string? family)
    {
        var face = FontCatalog.Resolve(family);

        Assert.NotNull(face);
        Assert.Same(FontCatalog.Resolve(FontCatalog.DefaultFamily), face);
    }

    [Fact]
    public void Resolve_returns_a_typeface_for_a_real_family()
    {
        var families = FontCatalog.Families;

        // Spot-check the ends and the middle instead of all ~400 installed families: the point is
        // that a name taken straight out of the dropdown always resolves, not that Skia can open
        // every font file on the machine.
        foreach (var index in new[] { 0, families.Count / 2, families.Count - 1 })
        {
            var entry = families[index];
            Assert.NotNull(FontCatalog.Resolve(entry.Family));
        }
    }

    [Fact]
    public void Resolve_caches_one_typeface_per_family()
    {
        // The overlay resolves on every redraw; without a cache that is a font-manager lookup and
        // a fresh native handle per frame.
        var family = FontCatalog.Families[0].Family;

        Assert.Same(FontCatalog.Resolve(family), FontCatalog.Resolve(family));
    }

    [Fact]
    public void Cached_typeface_survives_the_paint_that_used_it()
    {
        // The overlay builds a throwaway SKPaint per redraw and disposes it. If SKPaint disposal
        // released the shared typeface, the next frame would draw through a dead native handle —
        // which surfaces as a zero measurement or an access violation, never as a null reference.
        // Hence the width assertion: "not null" would pass happily on a stale handle.
        var family = FontCatalog.Families[0].Family;
        var typeface = FontCatalog.Resolve(family);

        for (var frame = 0; frame < 3; frame++)
        {
            using var paint = new SKPaint
            {
                IsAntialias = true,
                TextSize = 32f,
                Typeface = FontCatalog.Resolve(family),
            };

            var width = paint.MeasureText("Reshot");
            Assert.True(width > 0f, $"frame {frame} measured '{family}' as {width}px wide");
        }

        Assert.Same(typeface, FontCatalog.Resolve(family));

        using var after = new SKPaint { TextSize = 32f, Typeface = typeface };
        Assert.True(
            after.MeasureText("Reshot") > 0f,
            $"'{family}' stopped measuring once the paints that used it were disposed");
    }

    [Fact]
    public void At_least_one_family_reports_cyrillic_coverage()
    {
        // The probe is only meaningful where a Cyrillic-capable font is installed at all: a
        // stripped Server Core image or a container build agent can ship without one, and this
        // must not fail there. The inverse is deliberately never asserted either — which fonts
        // lack Cyrillic differs per machine and would turn this into a coin flip.
        var probe = FontCatalog.Resolve(FontCatalog.DefaultFamily);
        if (!probe.ContainsGlyph(0x0410))
            return;

        Assert.Contains(FontCatalog.Families, f => f.SupportsCyrillic);
    }

    [Fact]
    public void Warmup_is_safe_to_call_repeatedly()
    {
        // The app calls it at startup; a second call from a re-opened overlay must not rebuild or
        // throw.
        FontCatalog.Warmup();
        FontCatalog.Warmup();

        Assert.NotEmpty(FontCatalog.Families);
    }

    private static bool IsSorted(IReadOnlyList<string> names, StringComparer comparer)
    {
        for (var i = 1; i < names.Count; i++)
            if (comparer.Compare(names[i - 1], names[i]) > 0)
                return false;
        return true;
    }

    private static string FirstDisorder(IReadOnlyList<string> names)
    {
        for (var i = 1; i < names.Count; i++)
            if (StringComparer.OrdinalIgnoreCase.Compare(names[i - 1], names[i]) > 0)
                return $"'{names[i - 1]}' precedes '{names[i]}'";
        return "(none)";
    }
}
