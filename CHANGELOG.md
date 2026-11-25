# Changelog

User-visible changes per version. Versions are MAJOR.FEATURE.FIX (see AGENT.md).

## [Unreleased]

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
