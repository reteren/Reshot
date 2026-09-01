using Reshot.Core.Tools;
using SkiaSharp;
using Xunit;

namespace Reshot.Core.Tests;

// VectorObject.ComputeBounds() for VectorKind.Text is what the overlay draws its selection
// rectangle from and what it hit-tests against, so the box has to match the glyphs Skia actually
// paints. The old "~0.6 em per character" guess drifted badly on wide fonts and on anything that
// was not lowercase Latin, and now that the family is user-selectable it would drift further.
public class TextBoundsTests
{
    private static readonly SKPoint Origin = new(120f, 64f);

    private static VectorObject Text(string text, float size = 32f, string? family = null) => new()
    {
        Kind = VectorKind.Text,
        Text = text,
        FontSize = size,
        FontFamily = family ?? FontCatalog.DefaultFamily,
        P1 = Origin,
        Color = SKColors.White,
    };

    [Fact]
    public void Single_line_box_starts_exactly_at_P1()
    {
        var bounds = Text("Reshot").ComputeBounds();

        // DrawText places the first baseline at P1.Y - Ascent, which puts the top of the ink at
        // exactly P1.Y. Any other top and the selection rectangle floats off the glyphs.
        Assert.Equal(Origin.X, bounds.Left, 3);
        Assert.Equal(Origin.Y, bounds.Top, 3);
        Assert.True(bounds.Width > 0f, $"width was {bounds.Width}");
        Assert.True(bounds.Height > 0f, $"height was {bounds.Height}");
    }

    [Fact]
    public void Height_grows_with_the_line_count()
    {
        var one = Text("Reshot").ComputeBounds().Height;
        var two = Text("Reshot\nReshot").ComputeBounds().Height;
        var three = Text("Reshot\nReshot\nReshot").ComputeBounds().Height;

        Assert.True(two > one, $"two lines ({two}) should be taller than one ({one})");
        Assert.True(three > two, $"three lines ({three}) should be taller than two ({two})");
        // Roughly one line pitch per extra line. A band, not equality: the first and last lines
        // contribute ascent/descent rather than a full FontSpacing.
        Assert.InRange(three / one, 2.2f, 3.8f);
    }

    [Fact]
    public void Width_follows_the_widest_line_not_the_last_one()
    {
        var wide = Text("MMMMMMMMMMMM").ComputeBounds().Width;
        var narrow = Text("i").ComputeBounds().Width;
        var wideFirst = Text("MMMMMMMMMMMM\ni").ComputeBounds().Width;
        var wideLast = Text("i\nMMMMMMMMMMMM").ComputeBounds().Width;

        Assert.True(wide > narrow, $"a run of M ({wide}) should be wider than one i ({narrow})");
        // Whichever line happens to be widest drives the box, regardless of its position.
        Assert.InRange(wideFirst, wide * 0.99f, wide * 1.01f);
        Assert.InRange(wideLast, wide * 0.99f, wide * 1.01f);
    }

    [Fact]
    public void Doubling_the_font_size_roughly_doubles_the_box()
    {
        var small = Text("Reshot", 24f).ComputeBounds();
        var large = Text("Reshot", 48f).ComputeBounds();

        // A band rather than equality: hinting and rounded metrics keep the ratio near 2, not at it.
        Assert.InRange(large.Width / small.Width, 1.8f, 2.2f);
        Assert.InRange(large.Height / small.Height, 1.8f, 2.2f);
    }

    [Fact]
    public void A_longer_string_is_wider()
    {
        var shortText = Text("Re").ComputeBounds().Width;
        var longText = Text("Reshot capture overlay").ComputeBounds().Width;

        Assert.True(longText > shortText, $"long ({longText}) should exceed short ({shortText})");
    }

    [Theory]
    [InlineData("")]
    [InlineData(" ")]
    [InlineData("   ")]
    [InlineData("\n")]
    public void Empty_and_whitespace_text_still_produce_a_grabbable_box(string text)
    {
        var bounds = Text(text).ComputeBounds();

        // A text object with nothing typed into it yet is still selectable and draggable. A
        // zero-area rect would be impossible to hit with the mouse and invisible when selected.
        Assert.True(bounds.Width > 0f, $"width was {bounds.Width}");
        Assert.True(bounds.Height >= 16f, $"height was {bounds.Height} at font size 32");
        Assert.Equal(Origin.X, bounds.Left, 3);
        Assert.Equal(Origin.Y, bounds.Top, 3);
    }

    [Fact]
    public void Different_families_measure_differently()
    {
        const string Sample = "Reshot capture overlay";
        var baseline = Text(Sample).ComputeBounds().Width;

        // Metrically distinct faces that ship with Windows. Whichever of them this machine has is
        // enough to prove the family actually reaches the measurement instead of being ignored.
        var candidates = new[] { "Courier New", "Times New Roman", "Impact", "Consolas", "Georgia" };
        var installed = candidates
            .Where(c => FontCatalog.Families.Any(f =>
                string.Equals(f.Family, c, StringComparison.OrdinalIgnoreCase)))
            .ToList();

        // Nothing but the default family here — there is no second font to compare against, and
        // failing on that would only report the machine's font inventory, not a bug.
        if (installed.Count == 0)
            return;

        var widths = installed
            .Select(c => Text(Sample, family: c).ComputeBounds().Width)
            .ToList();

        // Half a pixel is far above measurement noise (an ignored family measures identically) and
        // far below any real metric difference, so this stays honest on a sparsely-fonted machine.
        Assert.Contains(widths, w => Math.Abs(w - baseline) > 0.5f);
    }

    [Fact]
    public void An_unknown_or_blank_family_measures_as_the_default()
    {
        var known = Text("Reshot", family: FontCatalog.DefaultFamily).ComputeBounds();
        var unknown = Text("Reshot", family: "NoSuchFontFamily_ZZZ").ComputeBounds();
        var blank = Text("Reshot", family: "").ComputeBounds();

        // Settings written by an older build, or by a machine with a font this one lacks, must
        // still measure — and must measure the same as what actually gets drawn.
        Assert.Equal(known.Width, unknown.Width, 3);
        Assert.Equal(known.Width, blank.Width, 3);
    }

    [Fact]
    public void New_text_objects_default_to_the_catalogue_default_family()
    {
        Assert.Equal(FontCatalog.DefaultFamily, new VectorObject().FontFamily);
    }
}
