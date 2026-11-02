"""Draws the installer's wizard images in Reshot's own look: the dark grey VGUI panel, the
app's cover art, a small bold caps title as on the app's panels, and the teal accent.

Run from the repository root after changing the icon:  python build/installer-art/make-art.py
Writes side-*.png (the welcome and finish pages) and small-*.png (the top right corner of the
other pages) at the scales Inno Setup picks from by DPI. Needs Pillow.
"""

from pathlib import Path

from PIL import Image, ImageDraw, ImageFont

ROOT = Path(__file__).resolve().parents[2]
OUT = Path(__file__).resolve().parent
ICON = ROOT / "crates" / "reshot" / "assets" / "reshot.ico"
BOLD = ROOT / "crates" / "reshot" / "assets" / "fonts" / "ReshotRoboto-Bold.ttf"
LIGHT = ROOT / "crates" / "reshot" / "assets" / "fonts" / "ReshotRoboto-Light.ttf"

# The page colour the installer script sets (WizardBackColor), a shade darker for the image
# panel, and the app's accent and rules (crates/reshot/src/theme.rs).
PAGE = (0x4B, 0x4B, 0x4B)
PANEL = (0x3A, 0x3A, 0x3A)
ACCENT = (0x3C, 0x98, 0x98)
RULE_DARK = (0x43, 0x43, 0x43)
RULE_LIGHT = (0xBB, 0xBA, 0xBA)
WHITE = (0xFF, 0xFF, 0xFF)

# Inno Setup's base sizes at 100 % and the scales it chooses between.
SIDE = (164, 314)
SMALL = (55, 55)
SCALES = (100, 125, 150, 175, 200, 250)


def cover() -> Image.Image:
    icon = Image.open(ICON)
    icon.size = max(icon.info["sizes"])
    return icon.convert("RGBA")


def side(scale: float, art: Image.Image) -> Image.Image:
    w, h = round(SIDE[0] * scale), round(SIDE[1] * scale)
    img = Image.new("RGB", (w, h), PANEL)
    d = ImageDraw.Draw(img)
    pad = round(14 * scale)
    # The cover, full width inside the padding, near the top.
    size = w - 2 * pad
    img.paste(art.resize((size, size), Image.LANCZOS), (pad, pad), art.resize((size, size), Image.LANCZOS))
    y = pad + size + round(14 * scale)
    # Engraved two-tone rule, as between groups in the app.
    d.line([(pad, y), (w - pad, y)], fill=RULE_DARK, width=max(1, round(scale)))
    d.line([(pad, y + max(1, round(scale))), (w - pad, y + max(1, round(scale)))], fill=RULE_LIGHT, width=max(1, round(scale)))
    y += round(12 * scale)
    title = ImageFont.truetype(str(BOLD), round(13 * scale))
    d.text((pad, y), "RESHOT", font=title, fill=WHITE)
    y += round(20 * scale)
    body = ImageFont.truetype(str(LIGHT), round(10.5 * scale))
    for line in ("Screenshots, drawing,", "OCR and recording", "for Windows."):
        d.text((pad, y), line, font=body, fill=(0xDD, 0xDD, 0xDD))
        y += round(14 * scale)
    # The accent down the edge that meets the page.
    d.rectangle([w - max(2, round(3 * scale)), 0, w, h], fill=ACCENT)
    return img


def small(scale: float, art: Image.Image) -> Image.Image:
    w, h = round(SMALL[0] * scale), round(SMALL[1] * scale)
    img = Image.new("RGB", (w, h), PAGE)
    icon = art.resize((w, h), Image.LANCZOS)
    img.paste(icon, (0, 0), icon)
    return img


def main() -> None:
    art = cover()
    for pct in SCALES:
        s = pct / 100
        side(s, art).save(OUT / f"side-{pct}.png", optimize=True)
        small(s, art).save(OUT / f"small-{pct}.png", optimize=True)
    print(f"wrote {len(SCALES) * 2} images to {OUT}")


if __name__ == "__main__":
    main()
