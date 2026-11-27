# Explore

The *explore lane* (View › Explore lane) shows thumbnails below the view: documents to start
from, random fractals and variants of the current document. Click a thumbnail to take its
document; Ctrl+Z goes back. The thumbnails render one after another in the background while
you keep working.

## Presets

Example documents to start from: classic places of the Mandelbrot set and the Burning Ship,
a Julia set, Newton's method, distance estimation, orbit traps, a kaleidoscope, a Droste
spiral, layers, a lit landscape and a deep zoom target.

## Random

**New batch** makes random fractals of the chosen kind:

- **Escape time**: the Mandelbrot set and its relatives (Multibrot, Burning Ship, Tricorn,
  Celtic, Buffalo, Phoenix, Spider, Manowar, Lambda …).
- **Julia sets** of those, with a seed near the edge of their set.
- **Root finding**: Newton, Nova, the magnets and polynomial root finding.
- **Lit reliefs**: escape-time fractals as lit height fields.
- **Any** mixes them.

Every random fractal gets a generated palette and is framed by zooming a few times into the
busiest part of a small preview, so the thumbnails show the fractal's edge with detail.
**Count** sets how many a batch makes. With **Filter** on, grainy, noisy or flat results are
made again with another seed (up to four tries each), which makes batches slower but better.

## Mutations

**Mutate** makes variants of the current document. Choose what changes:

- **Formula parameters**: the formula's numbers and the Julia seed move a little.
- **Colors**: a new palette, or the current one shifted in hue and saturation with another
  density and offset.
- **View**: moved, zoomed and turned (only views that still show detail).
- **Formula and Julia**: the Julia set of the view's center, or another formula of the same
  family in the same view.
- **Anything** picks one of these per variant.

**Strength** sets how far the variants go. A good way to explore is to mutate, take the
variant you like best, and mutate again.

## Snapshots

The camera button keeps the current document in the *Snapshots* strip, so you can come back
to it after trying mutations or edits. Snapshots last until RFract closes; save documents you
want to keep.
