# Coloring

Every point either escapes (the **outside**) or stays bounded until the iteration limit (the
**inside**). Both get their own coloring.

## Outside

| Method | Value |
|--------|-------|
| Smooth | Continuous iteration count: the iteration the point escaped at, plus how far beyond the bailout it landed. Convergent formulas (Newton, Magnet) use how fast the orbit settled. |
| Bands | The plain iteration count: one band per iteration. |
| Distance | The estimated distance to the set, relative to the pixel size (computed with automatic derivatives of the formula). Thin filaments stay visible. |
| Formula | A coloring algorithm (see below). |
| Solid | One color (with an opacity: 0 leaves the region transparent). |

The value goes through the **transfer** function (linear, square root, cube root,
logarithm, square), is multiplied by the **density** (gradient repetitions per unit) and
shifted by the **offset**, and then picks a color from the gradient. The gradient repeats.

![Distance estimation: bright edges, dark distance](images/example-distance.jpg)

## Inside

| Method | Value |
|--------|-------|
| Solid | One color (black by default). |
| Magnitude | The size of *z* after the last iteration. |
| Angle | The direction of *z* after the last iteration. |
| Formula | A coloring algorithm (see below). |

## Coloring algorithms

The *Formula* method runs a coloring algorithm along every orbit. Pick it under
*Algorithm*; its parameters follow.

| Algorithm | Colors by |
|-----------|-----------|
| Orbit trap | The closest approach of the orbit to a point, line, circle, cross, square, spiral, astroid or heart (distance, iteration, angle or size there) |
| Triangle inequality average | How ‖z²‖ + ‖c‖ compares with ‖z² + c‖ along the orbit, smoothed |
| Curvature average | The average turning angle of the orbit |
| Stripe average | The average of sin(stripes · angle of z): stripes that follow the orbit |
| Exponential smoothing | A sum of exp(−‖z‖) (escaping) or exp(−1/‖Δz‖) (converging orbits) |
| Binary decomposition | The sign of Im(z) at escape |
| Field lines | The angle of z at escape, repeated |
| Decomposition (angle) | The angle of z at the end |
| Gaussian integer | The distance of the orbit to the nearest lattice point |
| Pickover stalks | The distance of the orbit to two axes: thin stalks |
| Epsilon cross | The first iteration close to an axis |
| Closest approach (bof60) | The smallest ‖z‖ of the orbit |
| Iteration of closest approach (bof61) | The iteration of the smallest ‖z‖ |

Algorithms can mark points as *solid* (for example outside a trap): those get the solid color
below the gradient, which can also be transparent.

![Orbit traps: a cross outside, a circle inside](images/example-traps.jpg)

Coloring algorithms are formulas in UFM syntax (`formulas/rfract/rfract.ucl`): `init:` runs
once, `loop:` after every iteration, `final:` at the end; they read `#z` and write `#index`
(the gradient position) or `#solid`.

## The gradient editor

Click a gradient in the inspector to edit it.

![The gradient editor](images/gradient-editor.png){: .half}

- **The strip**: drag a marker to move its color, click the strip to add a color there,
  right-click or double-click a marker to remove it.
- **Presets** and saved palettes; save the current colors under a name.
- **Random** makes a new palette with the generator's settings.
- **Generator**: harmony (analogous, complementary, triadic, …), number of colors, hue,
  saturation and lightness ranges, order, hardness and seamless repetition.
- **Adjust**: hue, saturation, lightness, contrast and hardness of the whole palette.
- **Tools**: reverse, shift, mirror, repeat, space evenly, sort by lightness, invert
  lightness, hard steps, smooth blends.
- **Opacity**: points with a position and an opacity make parts of the gradient transparent,
  so the layers below show through; the strip shows a checkerboard there.
- **Import** reads gradients from `.ugr` (several per file, with opacity), `.map`, `.ggr`,
  `.gpl` and `.cpt` files; **Export** writes `.ugr`, `.map` or `.ggr`.
- **Blend**: smooth (eased) or linear blends. Colors always blend in the perceptual OKLab
  space, so gradients keep even brightness steps.

## Color cycling

The play button in the top bar (or *View › Cycle colors*) turns all gradients in the view;
the speed and direction are in the View menu. The document keeps its colors: cycling is only
for looking (and animations will use it later).
