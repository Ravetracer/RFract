# Coloring

Every point either escapes (the **outside**) or stays bounded until the iteration limit (the
**inside**). Both get their own coloring.

## Outside

| Method | Value |
|--------|-------|
| Smooth | Continuous iteration count: the iteration the point escaped at, plus how far beyond the bailout it landed. Convergent formulas (Newton, Magnet) use how fast the orbit settled. |
| Bands | The plain iteration count: one band per iteration. |
| Distance | The estimated distance to the set, relative to the pixel size (computed with automatic derivatives of the formula). Thin filaments stay visible. |
| Solid | One color. |

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
- **Blend**: smooth (eased) or linear blends. Colors always blend in the perceptual OKLab
  space, so gradients keep even brightness steps.
