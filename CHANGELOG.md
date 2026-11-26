# Changelog

User-visible changes per version. Versions are MAJOR.FEATURE.FIX (see AGENT.md).

## [Unreleased]

### Added

- Views at any depth: the zoom is kept as a number with an extended exponent and the center
  as exact decimal text; panning and zooming stay exact below 1e-308. The *Zoom* field takes
  text such as `1e500`.
- Deep zoom by perturbation on the GPU and the CPU: a reference orbit in arbitrary precision
  at the view center, every sample iterates its difference with an extended exponent and
  rebasing. The perturbed iteration is derived from the formula itself (sums, products,
  integer powers, conj, abs, real/imag parts), so Mandelbrot powers, Burning Ship, Celtic,
  Buffalo, Tricorn and similar formulas zoom past 1e-308. The reference orbit stays while
  you navigate near it. The status bar shows *perturbation*; it warns only about layers that
  cannot be perturbed.
- Bilinear approximation (BLA) for deep zooms: linear steps derived from the formula skip
  hundreds to thousands of iterations where the difference to the reference stays small
  (about 20× faster at 1e-30 with 50 000 iterations).
- New reference orbits are computed in the background while the old one keeps rendering
  (status bar: *reference orbit…*); a reference shorter than the iteration limit stays usable
  (rebasing), so changing the iterations does not wait either.

## [0.4.1] - 2026-11-26

### Added

- UFM classes: class libraries (.ulb) from the formula folders, objects, methods,
  inheritance, base class and static calls, object fields and arrays, `return`, object
  parameters (plugins) with their own parameters, chosen in the inspector. Objects are
  compiled into plain formula code, so class formulas render on the GPU.
- RFract's own versions of common standard classes (Mandelbrot, Julia, Pixel, Newton, Smooth,
  Gradient, Default, Orbit Traps, Direct Orbit Traps, a trap mode, 18 trap shapes, Exponential
  Smoothing, Distance Estimator, Height Transfer).
- Color function parameters (merge modes, `@f(color1, color2)`).
- Arrays of objects (`TrapShape m_Elements[10]`, `setLength`, `length`): objects stored at
  known indexes, counting loops that make or use objects are unrolled, elements chosen by an
  index known only while rendering are picked at run time; objects in conditions test for
  null (`if m_Elements[j]`). Merge classes such as `TransformMerge` and `TrapShapeMerge`
  compile.
- Object arrays sized by an int parameter with a maximum; loops up to such a parameter unroll
  to the maximum and check the condition while rendering.
- Objects chosen while rendering: variables and array elements that refer to different
  objects depending on conditions, indexes from parameters or the branch a method returns
  from (was an error).
- Method calls on array elements as statements (`m_Elements[j].Init(pz)`).
- Image parameters: `Image param` chooses a PNG or JPEG file in the inspector; formulas read
  it through image objects (`getColor`, `getPixel`, `getWidth`, `getHeight`, `getEmpty`) on
  the CPU and the GPU. Image classes of class libraries (`ImageImport`) work.
- Functions declared in formula sections (`complex func f(…) … endfunc`); arrays are passed
  to functions and methods by reference; `complex b[n] = a` and `b = a` copy arrays.
- Assignments inside expressions (`(x = sqrt(y)) < 1`, `abs(r = random(r))`,
  `until (i = i + 1) >= 10`); a single `=` inside parentheses was a comparison before, which
  computed such formulas wrongly.
- Arithmetic on colors: `+`, `-`, `*`, `/` channel by channel, numbers apply to all four
  channels (`c1 * t + c2 * (1 - t)`).
- The *Image* coloring: an image file looked up by the final z, the pixel or the orbit's
  closest approach (image trap), with size, angle, proportions and tiling.

### Fixed

- `0^0` is 1 in complex and real powers (was 0).

## [0.4.0] - 2026-11-25

Stage 3: formula language compatibility (UFM and FractInt).

### Added

- UFM formulas: arrays (fixed and dynamic), the `color` type with color parameters and color
  functions, direct coloring through `#color`, merge functions, `random(seed)`,
  `#randomrange`, assignments to parameters, FractInt syntax inside UFM files and the
  `$define`/`$ifdef` preprocessor, several statements per line.
- Coloring *Direct color (hue by angle)*.
- Formula folders (File › Preferences): RFract indexes the user's .ufm, .frm, .ucl and .uxf
  files in the background and lists them in the browser under *User formulas*, in the coloring
  picker and in *Add a transformation*; formulas compile when they are picked.
- FractInt .frm formula files (symmetry suffixes ignored, `rotskew`, `magxmag`, variables that
  start at zero, `f(a, b)` as `f((a, b))`, `end if`, a final assignment as the bailout).
- rfr-cli: `--formula-dir <folder>` makes the formulas of a folder available to every command.
- Parameter files (.upr): File › Open lists their images; layers, view, formulas, colorings,
  transformations and gradients are converted, standard formulas map onto built-in ones, and
  an *Import notes* window lists what did not carry over. rfr-cli renders .upr images
  (`--entry`) and checks whole folders (`upr --check`, optional thumbnails).
- Formula *Pixel (plane only)* and colorings *Iteration count* and *Value of z*.
- FractInt types: 40 escape-time types of FractInt (lambda, fn types, phoenix, Barnsley,
  Man-o-war, spider, Marks, Halley, complex Newton, popcorn Julia, tetrate, unity, Lyapunov,
  …) as formulas with FractInt's parameter layout.
- FractInt parameter files (.par): types, parameters, functions, view (corners with rotation,
  center-mag), iteration limit, inside color, palettes (`colors=` and .map files, FractInt's
  default VGA palette otherwise), `type=formula` from the formula folders. rfr-cli
  `params --check` covers .par files too.
- UFM functions `isNaN` and `isInf`.
- Formula editor: *Source* shows a formula's entry, checks it while typing and saves it back
  into its file (the view follows); built-in formulas can be copied into the formula folder;
  **+** above the formula list creates new formulas from templates.
- The inspector shows why a formula does not compile.
- Color parameters in the inspector (color picker).

### Changed

- Formula parameters: up to 4096 values per layer (was 256).

## [0.3.0] - 2026-11-25

Stage 2: layers, coloring, transformations, gradients.

### Added

- Layers: any number of fractal layers and groups with 18 merge modes (Normal, Multiply,
  Screen, Overlay, Soft/Hard light, Color dodge/burn, Darken, Lighten, Difference, Exclusion,
  Add, Subtract, Hue, Saturation, Color, Luminosity), opacity and visibility; fractal masks
  (brightness or alpha, invertible) for layers and groups; layer list with add, duplicate,
  delete, move, group and ungroup.
- 13 coloring algorithms as formulas: orbit traps (point, line, circle, cross, square, spiral,
  astroid, heart), triangle inequality average, curvature and stripe averages, exponential
  smoothing, binary decomposition, field lines, decomposition, Gaussian integer, Pickover
  stalks, epsilon cross, closest approach and its iteration.
- 11 transformations: affine, mirror, kaleidoscope, polar coordinates, circle inversion,
  Möbius, Riemann sphere, tiling, twirl, ripple, Droste spiral; several per layer, in order.
- Gradients with an opacity curve; solid colors with opacity; background color with opacity
  (transparent PNG export, checkerboard in the view).
- Gradient import (.ugr, .map, .ggr, .gpl, .cpt) and export (.ugr, .map, .ggr).
- Color cycling in the view (View › Cycle colors, play button).
- rfr-cli formulas --check also checks coloring (.ucl) and transformation (.uxf) files.

### Changed

- Documents are version 2 (version 1 files open unchanged).

## [0.2.0] - 2026-11-25

Stage 1: escape-time core and viewer.

### Added

- Formula language with two front ends: UFM syntax (RFract's own formulas) and FractalPark
  FRM-like v1; one typed IR, compiled for the CPU and generated as WGSL for the GPU.
- 552 built-in formulas: 18 RFract formulas (Mandelbrot, Multibrot, Burning Ship, Tricorn,
  Celtic, Buffalo, Perpendicular, Phoenix, Lambda, Magnet I/II, Newton, Nova, Spider, Manowar,
  Barnsley 1–3) and the 534 FractalPark formulas.
- GPU renderer with progressive anti-aliasing, automatic double-float precision for deep views
  (clean to about 10¹² at full HD) and automatic derivatives for distance estimation; CPU
  reference renderer with the same results.
- Coloring: smooth iteration count (divergent and convergent formulas), bands, distance
  estimation, solid; inside solid, magnitude or angle; transfer, density and offset.
- Gradients blended in OKLab with presets; gradient editor with generator, adjustments and
  tools (from RMandelbulb3D).
- View navigation: drag, wheel zoom at the cursor, box zoom, rotation, double-click zoom,
  Julia sets by Alt+click or J, reset view.
- Formula browser with search, inspector for parameters, iterations, Julia mode, coloring,
  view and image size; undo/redo for every change including the view.
- Documents (.rfr), PNG export with the embedded document (opens again as its settings).
- rfr-cli render (GPU, or --cpu), formulas, formulas --check.

## [0.1.0] - 2026-11-25

Stage 0: foundation and distribution.

### Added

- Program shell: window with the final panel layout (layers, inspector, timeline, explore
  lane, viewport), preferences (UI scale, font size), help window with the embedded manual.
- `rfr-cli info`: version, graphics card, denoiser and settings folders; `rfr-cli --version`.
- Release packages for Linux (tar.gz, AppImage), Windows (zip) and macOS (arm64 and x86_64,
  unsigned) built by GitHub Actions, with the denoiser and the PDF manual.
- FractalPark's published formula library (534 formulas, MIT) bundled as source for the
  built-in formulas; credited in NOTICE, THIRD_PARTY_NOTICES.md and Help › About.
