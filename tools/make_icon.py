#!/usr/bin/env python3
# SPDX-License-Identifier: LGPL-3.0-or-later
"""Draws the program icon and the manual cover (pure Python + Pillow, no renderer needed).

    python3 tools/make_icon.py

Writes crates/rfr-app/assets/icon.png (256 px), icon.ico (16-256 px) and
docs/manual/images/cover.jpg. Slow (pure Python): about a minute.
"""

import math
from pathlib import Path

from PIL import Image, ImageDraw, ImageFilter

ROOT = Path(__file__).resolve().parent.parent

# Gradient stops (position, r, g, b): deep blue, cyan, white, amber, dark red, back to blue.
STOPS = [
    (0.00, 8, 12, 40),
    (0.18, 30, 90, 200),
    (0.36, 120, 220, 255),
    (0.50, 255, 250, 235),
    (0.66, 255, 170, 40),
    (0.82, 140, 20, 40),
    (1.00, 8, 12, 40),
]


def gradient(t: float) -> tuple[float, float, float]:
    t %= 1.0
    for (p0, *c0), (p1, *c1) in zip(STOPS, STOPS[1:]):
        if t <= p1:
            u = (t - p0) / (p1 - p0)
            u = u * u * (3 - 2 * u)
            return tuple(a + (b - a) * u for a, b in zip(c0, c1))
    return tuple(STOPS[-1][1:])


def smooth_iter(cx: float, cy: float, max_iter: int):
    """Continuous escape count of the Mandelbrot set, None inside."""
    x = y = 0.0
    for i in range(max_iter):
        xx, yy = x * x, y * y
        if xx + yy > 1e6:
            return i + 1 - math.log2(math.log(xx + yy) * 0.5)
        y = 2 * x * y + cy
        x = xx - yy + cx
    return None


def render(w: int, h: int, cx: float, cy: float, span: float, rot: float, max_iter: int, ss: int, density: float, offset: float) -> Image.Image:
    img = Image.new("RGB", (w, h))
    px = img.load()
    scale = span / w
    ca, sa = math.cos(rot), math.sin(rot)
    for j in range(h):
        for i in range(w):
            acc = [0.0, 0.0, 0.0]
            for sj in range(ss):
                for si in range(ss):
                    u = (i + (si + 0.5) / ss - w / 2) * scale
                    v = (j + (sj + 0.5) / ss - h / 2) * scale
                    n = smooth_iter(cx + u * ca - v * sa, cy + u * sa + v * ca, max_iter)
                    c = (2, 3, 10) if n is None else gradient(math.log(n + 1) * density + offset)
                    acc[0] += c[0]
                    acc[1] += c[1]
                    acc[2] += c[2]
            k = ss * ss
            px[i, j] = (int(acc[0] / k), int(acc[1] / k), int(acc[2] / k))
    return img


def icon():
    size = 256
    art = render(size, size, -0.7453, 0.1127, 0.012, 0.6, 400, 3, 0.9, 0.15)
    mask = Image.new("L", (size, size), 0)
    ImageDraw.Draw(mask).rounded_rectangle((8, 8, size - 9, size - 9), radius=52, fill=255)
    out = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    out.paste(art, (0, 0), mask.filter(ImageFilter.GaussianBlur(0.8)))
    assets = ROOT / "crates" / "rfr-app" / "assets"
    assets.mkdir(parents=True, exist_ok=True)
    out.save(assets / "icon.png")
    out.save(assets / "icon.ico", sizes=[(16, 16), (24, 24), (32, 32), (48, 48), (64, 64), (128, 128), (256, 256)])
    print(f"wrote {assets / 'icon.png'}, icon.ico")


def cover():
    w, h = 1200, 800
    art = render(w, h, -0.743643887, 0.131825904, 0.0022, 0.3, 700, 2, 0.75, 0.55)
    dest = ROOT / "docs" / "manual" / "images" / "cover.jpg"
    dest.parent.mkdir(parents=True, exist_ok=True)
    art.save(dest, quality=90)
    print(f"wrote {dest}")


if __name__ == "__main__":
    icon()
    cover()
