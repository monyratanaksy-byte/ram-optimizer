"""Draws the app icon and writes every size the app and .exe need.

Run from the repo root:  python3 scripts/make_icons.py
Needs Pillow. Outputs go to crates/app/assets/.
"""

from pathlib import Path

from PIL import Image, ImageChops, ImageDraw, ImageFilter

OUT = Path(__file__).resolve().parent.parent / "crates" / "app" / "assets"

FACE_TOP = (240, 241, 244)
FACE_BOTTOM = (218, 220, 225)
WELL = (205, 208, 214)
WELL_SHADE = (176, 180, 188)
INK = (27, 28, 31)
ACCENT = (217, 138, 43)

# Fraction of the gauge that is filled (from 12 o'clock, clockwise).
FILL = 0.72
SUPERSAMPLE = 8


def lerp(a, b, t):
    return tuple(round(x + (y - x) * t) for x, y in zip(a, b))


def draw_icon(size: int) -> Image.Image:
    s = size * SUPERSAMPLE
    img = Image.new("RGBA", (s, s), (0, 0, 0, 0))

    # Small sizes use the whole canvas; large ones leave room for a soft shadow.
    margin = 0.0 if size <= 32 else 0.06
    x0 = y0 = s * margin
    x1 = y1 = s * (1 - margin)
    side = x1 - x0
    radius = side * 0.24

    if size > 32:
        shadow = Image.new("L", (s, s), 0)
        ImageDraw.Draw(shadow).rounded_rectangle(
            (x0, y0 + side * 0.03, x1, y1 + side * 0.03), radius, fill=90
        )
        shadow = shadow.filter(ImageFilter.GaussianBlur(side * 0.035))
        img.paste((0, 0, 0, 255), (0, 0), shadow)

    # Face: rounded square with a gentle top-to-bottom gradient.
    gradient = Image.new("RGBA", (s, s))
    gdraw = ImageDraw.Draw(gradient)
    for y in range(s):
        t = min(max((y - y0) / side, 0), 1)
        gdraw.line([(0, y), (s, y)], fill=lerp(FACE_TOP, FACE_BOTTOM, t) + (255,))
    mask = Image.new("L", (s, s), 0)
    ImageDraw.Draw(mask).rounded_rectangle((x0, y0, x1, y1), radius, fill=255)
    img.paste(gradient, (0, 0), mask)

    cx = cy = s / 2
    ring_r = side * 0.30
    stroke = side * 0.115

    # Pressed-in ring well, darker toward the top-left like the app's insets.
    well = Image.new("RGBA", (s, s), (0, 0, 0, 0))
    wdraw = ImageDraw.Draw(well)
    outer = ring_r + stroke / 2 + side * 0.02
    inner = ring_r - stroke / 2 - side * 0.02
    wdraw.ellipse((cx - outer, cy - outer, cx + outer, cy + outer), fill=WELL + (255,))
    shade = Image.new("L", (s, s), 0)
    sdraw = ImageDraw.Draw(shade)
    off = side * 0.018
    sdraw.ellipse((cx - outer, cy - outer, cx + outer, cy + outer), fill=255)
    sdraw.ellipse((cx - outer + off, cy - outer + off, cx + outer + off, cy + outer + off), fill=0)
    shade = shade.filter(ImageFilter.GaussianBlur(side * 0.01))
    well.paste(WELL_SHADE + (255,), (0, 0), shade)
    ring_mask = Image.new("L", (s, s), 0)
    rdraw = ImageDraw.Draw(ring_mask)
    rdraw.ellipse((cx - outer, cy - outer, cx + outer, cy + outer), fill=255)
    rdraw.ellipse((cx - inner, cy - inner, cx + inner, cy + inner), fill=0)
    img.paste(well, (0, 0), ImageChops.multiply(ring_mask, well.getchannel("A")))

    # Gauge arc with round caps.
    draw = ImageDraw.Draw(img)
    box = (cx - ring_r - stroke / 2, cy - ring_r - stroke / 2, cx + ring_r + stroke / 2, cy + ring_r + stroke / 2)
    start, end = -90, -90 + 360 * FILL
    draw.arc(box, start, end, fill=INK + (255,), width=round(stroke))
    import math

    def cap(angle_deg, color, scale=1.0):
        a = math.radians(angle_deg)
        px, py = cx + ring_r * math.cos(a), cy + ring_r * math.sin(a)
        r = stroke / 2 * scale
        draw.ellipse((px - r, py - r, px + r, py + r), fill=color + (255,))

    cap(start, INK)
    cap(end, INK)
    # Amber marker at the tip of the gauge.
    cap(end, ACCENT, 0.62)

    return img.resize((size, size), Image.LANCZOS)


def main():
    OUT.mkdir(parents=True, exist_ok=True)
    sizes = [16, 20, 24, 32, 40, 48, 64, 128, 256]
    images = [draw_icon(n) for n in sizes]
    images[-1].save(OUT / "icon.ico", sizes=[(n, n) for n in sizes], append_images=images[:-1])
    draw_icon(256).save(OUT / "icon.png")
    draw_icon(1024).save(OUT / "icon-1024.png")
    # Raw RGBA for the tray icon, so the app needs no image decoder.
    tray = draw_icon(32)
    (OUT / "tray-32.rgba").write_bytes(tray.tobytes())
    print("wrote", ", ".join(p.name for p in sorted(OUT.iterdir())))


if __name__ == "__main__":
    main()
