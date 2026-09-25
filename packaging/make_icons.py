"""Renders the omnidl icon (blue rounded square, white download arrow).

Writes the Windows icon, the browser extension icons, the raw RGBA window
icon the app embeds and the master for the macOS icon (packaging/macos/bundle.sh
turns it into omnidl.icns). Needs Pillow. Run from the repository root:

    python packaging/make_icons.py
"""

from pathlib import Path

from PIL import Image, ImageDraw

ROOT = Path(__file__).resolve().parent.parent
S = 1024  # drawing size; everything is scaled down from here


def render() -> Image.Image:
    img = Image.new("RGBA", (S, S), (0, 0, 0, 0))

    # Background: rounded square with a soft top-to-bottom blue gradient.
    inset, radius = 48, 224
    mask = Image.new("L", (S, S), 0)
    ImageDraw.Draw(mask).rounded_rectangle((inset, inset, S - inset, S - inset), radius, fill=255)
    top, bottom = (48, 150, 255), (0, 100, 235)
    gradient = Image.new("RGBA", (S, S))
    gd = ImageDraw.Draw(gradient)
    for y in range(S):
        t = y / (S - 1)
        gd.line([(0, y), (S, y)], fill=tuple(round(a + (b - a) * t) for a, b in zip(top, bottom)) + (255,))
    img.paste(gradient, (0, 0), mask)

    # Arrow: rounded stem plus head, then a tray line underneath.
    d = ImageDraw.Draw(img)
    cx = S // 2
    stem_w = 150
    d.rounded_rectangle((cx - stem_w // 2, 190, cx + stem_w // 2, 560), stem_w // 2, fill="white")
    d.polygon([(cx - 270, 510), (cx + 270, 510), (cx, 765)], fill="white")
    d.rounded_rectangle((cx - 300, 820, cx + 300, 880), 30, fill=(255, 255, 255, 235))
    return img


def main() -> None:
    big = render()
    sized = lambda n: big.resize((n, n), Image.LANCZOS)

    assets = ROOT / "assets"
    assets.mkdir(exist_ok=True)
    sizes = [16, 20, 24, 32, 40, 48, 64, 128, 256]
    sized(256).save(assets / "omnidl.ico", sizes=[(n, n) for n in sizes])
    sized(512).save(assets / "icon-512.png")
    (assets / "icon-128.rgba").write_bytes(sized(128).tobytes())

    icons = ROOT / "extension" / "icons"
    icons.mkdir(parents=True, exist_ok=True)
    for n in (16, 32, 48, 128):
        sized(n).save(icons / f"{n}.png")

    # macOS draws app icons on an 824 px body inside the 1024 px canvas; ours
    # fills 928 px, so shrink it to match the Dock and Launchpad neighbours.
    body = S - 2 * 48
    n = round(S * 824 / body)
    mac = Image.new("RGBA", (S, S), (0, 0, 0, 0))
    mac.paste(sized(n), ((S - n) // 2, (S - n) // 2))
    mac.save(ROOT / "packaging" / "macos" / "icon-1024.png", optimize=True)
    print("Icons geschrieben.")


if __name__ == "__main__":
    main()
