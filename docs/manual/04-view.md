# The view

## Navigation

| Input | Action |
|-------|--------|
| Drag | Move the view |
| Mouse wheel | Zoom in or out at the cursor |
| Shift+drag, right drag | Zoom into the box you draw |
| Ctrl+drag | Rotate the view around its center |
| Double-click | Zoom in by two at the point |
| Right click | Zoom out by two |
| Alt+click | Julia set of the point (Alt+click again goes back) |
| J | Julia set of the point under the cursor, or back |
| Home | Back to the formula's suggested view |

While you navigate, the preview renders at a lower resolution (Preferences › Preview › While
navigating) and switches back to full resolution once you stop.

The *View* section of the inspector shows the center (as text, with as many digits as the
zoom needs), the magnification and the rotation. Type exact coordinates there to go to a known
place, and any magnification such as `1e500` (the status bar shows it as `zoom 1.500e30×`).

## Julia sets

Formulas with a Mandelbrot/Julia switch iterate in two ways:

- **Mandelbrot mode**: every point of the plane is the formula's constant *c*; the image is a
  map of all Julia sets.
- **Julia mode**: *c* is one fixed seed and every point is the start value; the image is the
  Julia set of that seed.

Alt+click a point (or press **J** over it) to see its Julia set; the seed is the point you
clicked. **Pick a Julia seed in the view** in the inspector does the same with the next click.
Switching back restores the view you left. The seed can also be typed or dragged in the
inspector.

![Julia set of the seed (-0.8, 0.156)](images/example-julia.jpg)

## Precision

The graphics card computes with 32-bit floating point numbers while that is precise enough.
Zoom in further (beyond a few thousand times at full HD) and RFract switches to double-float arithmetic (two 32-bit
numbers per value, about 48 bits), which stays clean to about 10¹² at full HD. The status bar
shows the arithmetic in use (*f32*, *f32 + derivatives* for distance estimation,
*double-float* or *perturbation*).

## Deep zoom

Beyond double precision RFract renders by **perturbation**: one reference orbit at the view
center is computed with as many digits as the depth needs, and every pixel follows only its
small difference to it, with numbers whose exponent goes far beyond 10⁻³⁰⁸. There is no
depth limit: views at 10⁻¹⁰⁰⁰ work like any other. While you navigate, the reference orbit
stays the same as long as it is near and precise enough, so zooming does not wait for it;
when you move far away, a new one is computed in the background (the status bar shows
*reference orbit…*) while the old one keeps rendering.

Deep views need many iterations: raise *Iterations* until the black regions show detail (the
center of a spiral often needs tens of thousands). Most of them are skipped: where the
difference to the reference stays small, RFract jumps over hundreds or thousands of
iterations at once with precomputed linear steps (*bilinear approximation*), so a full HD
image at 10⁻³⁰ with 50 000 iterations takes seconds. Coloring algorithms that look at every
iteration (orbit traps and the like) need all of them and render slower at depth.

Perturbation works for formulas built from sums, products, quotients, whole-number powers,
`conj`, `abs`, `real`, `imag` and `flip` with variables that do not depend on conditions in the
loop: Mandelbrot and its powers, Burning Ship, Tricorn, Celtic, Buffalo, Perpendicular, many
user formulas. RFract derives the perturbed formula itself; nothing has to be written twice.

!!! note
    Layers whose formula cannot be perturbed (functions such as `sin` or `exp`, conditions
    that depend on the orbit, integer variables) or that use transformations show rounding
    steps beyond double precision; the status bar warns and its hover text names them.
    Distance estimation colors by the smooth iteration count in double-float and perturbation
    views.
