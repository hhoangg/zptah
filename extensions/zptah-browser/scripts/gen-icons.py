"""Regenerates icons/*.png from the zptah app logo (assets/native/monocode.png).

Run by hand when the logo changes (needs Pillow); the build only copies the committed PNGs.
The artwork is the app icon mark on black, cropped tighter than the macOS icon so it stays
readable at toolbar sizes, with rounded corners.
"""
from pathlib import Path
from PIL import Image, ImageDraw

ROOT = Path(__file__).resolve().parents[1]
SRC = ROOT.parents[1] / "assets/native/monocode.png"
SIZES = (16, 32, 48, 128)
CROP = (110, 110, 914, 914)  # mark bounds (143..881) plus a small margin
SS = 8  # supersampling for the corner mask


def render(size: int) -> Image.Image:
    mark = Image.open(SRC).convert("RGBA").crop(CROP)
    base = Image.new("RGBA", mark.size, (0, 0, 0, 255))
    base.alpha_composite(mark)
    img = base.resize((size, size), Image.LANCZOS)
    mask = Image.new("L", (size * SS, size * SS), 0)
    ImageDraw.Draw(mask).rounded_rectangle((0, 0, size * SS - 1, size * SS - 1), radius=int(size * SS * 0.22), fill=255)
    img.putalpha(mask.resize((size, size), Image.LANCZOS))
    return img


if __name__ == "__main__":
    (ROOT / "icons").mkdir(exist_ok=True)
    for s in SIZES:
        render(s).save(ROOT / f"icons/icon-{s}.png", optimize=True)
    # Web Store assets: the 128 store icon and the 440x280 small promo tile (logo on black, no alpha).
    (ROOT / "store").mkdir(exist_ok=True)
    render(128).save(ROOT / "store/icon-128.png", optimize=True)
    tile = Image.new("RGB", (440, 280), (0, 0, 0))
    logo = render(256)
    tile.paste(logo, ((440 - 256) // 2, (280 - 256) // 2), logo)
    tile.save(ROOT / "store/promo-tile-440x280.png", optimize=True)
    print("wrote", ", ".join(f"icon-{s}.png" for s in SIZES))
