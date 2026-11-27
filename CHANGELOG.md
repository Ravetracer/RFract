# Changelog

User-visible changes per version. Versions are MAJOR.FEATURE.FIX (see AGENT.md).

## [Unreleased]

### Added

- Periodicity checking and the main cardioid and bulb test: with a solid inside color,
  points inside the set stop early, so high iteration limits render much faster on views
  with large inside areas; the image stays the same.
- Automatic iteration count (*Auto* next to *Iterations*): the limit rises when the view
  needs more.
- Light sources (*Lights* in the inspector): points in the plane with a visible glow and lens
  flares (core, haze, star rays, streak, ghosts, ring, sparkles, lens dirt) with presets;
  drag them in the view. Lit layers and 3D terrains can be lit by them as point lights
  (*Document lights*), as can 3D objects; over a terrain or an object they hang at their
  height, seen through the camera, and fade out behind them. Lights can fall off with
  distance (*Reach*), and flare ghosts can be edited one by one.

### Removed

- The lens flare of the effects (ghosts of bright pixels); flares now come from lights.

## [0.10.0] - 2026-11-27

### Added

- Explore lane (*View › Explore lane*): presets, batches of random fractals (escape time,
  Julia sets, root finding, lit reliefs) framed at their edge, mutations of the current
  document (parameters, colors, view, formula), a quality filter and snapshots.
- Posters: images above 8K UHD render in tiles (export, command line, render queue) and
  are written a band at a time, so 20000 px and more fit on any graphics card.
- Render queue: settings per job (name, size, samples, frames, output file).
- Adaptive sampling (*Image › Adaptive*, on by default): pixels stop taking samples once
  they and their neighbours are smooth; renders with many samples are about three times
  faster.
- CPU renderer: guessing fills blocks inside the set from their border, so inside areas
  render two to three times faster without a graphics card (`rfr-cli --no-guess` computes every
  pixel).
- Manual: how-tos (deep zoom movie, poster, landscape fly-over, music video, exploring,
  parameter files, overnight renders); the introduction describes the current program.

## [0.9.0] - 2026-11-27

### Added

- Animation (core and command line): keyframes are whole documents, every number blends
  between them (monotone cubic, distances on a log scale, angles the short way); the view
  zooms at an even pace in log space with the target staying put, exact at any depth;
  timing per keyframe (frames, easing) or steady (length, ease in and out); palette rotation;
  motion blur (shutter); `rfr-cli animate` writes MP4 (H.264, H.265, AV1), MOV (ProRes),
  WebM, GIF through ffmpeg, or PNG frames. In the app: *View › Timeline* with keyframes,
  scrubbing, playback in the view, timing, palette rotation, motion blur and *Export
  animation…*. *Zoom sequence*: a straight deep zoom composed from one image per doubling
  of the zoom (many times faster for long zooms).
- Fly mode (*View › Fly mode*, F): hold the left mouse button to fly into the fractal toward
  the cursor, the right one to fly out, the wheel sets the speed; the speed ramps up and
  down smoothly.
- Flight recording in the timeline: the view and the 3D cameras while navigating become
  keyframes (as few as the *Tolerance* allows) with the recorded timing (*As flown*, speed
  changes smoothed); the *Cameraman* smooths jerky moves.
- Drivers: document values computed by expressions in every frame (time, keyframe position,
  the keyframed value, other values, sound levels), with a curve per driver in the timeline.
- Sound: a sound file under exported movies; drivers can follow its loudness, bass, mids and
  treble (analysed once with ffmpeg, stored in the document).
- Render queue (*File › Render queue…*): images and animations rendered one after the other,
  kept between sessions; `rfr-cli batch` runs a queue file or a list of documents.

## [0.8.0] - 2026-11-27

### Changed

- 3D terrain: its own *Terrain height* (a share of the image height) instead of the relief,
  with heights normalized per source; before, the set rose as a column thousands of pixels
  high and the walls looked like streaks.

### Fixed

- 3D terrain: the ground no longer moves between render passes (its heights stay at the
  pixel centers), which left translucent, blurred stair steps along edges seen from low
  cameras; rays that skim the ground no longer run out of steps on large images.

### Added

- Effects on the finished image (inspector section *Effects*): exposure, tone mapping
  (Reinhard, ACES, filmic, AgX), contrast, saturation, white balance, split toning, bloom,
  streaks, sharpening, lens distortion, chromatic aberration, vignette and film grain; the
  preview (GPU) and the export (GPU or CPU, command line) show the same.
- Effects: god rays toward a light position, lens flares (ghosts and halo), levels (black
  and white point, midtones), a five-point tone curve and spectral chromatic aberration.
- *Denoise exports* (Effects): exports and command line renders through Intel Open Image
  Denoise, with an amount; with lit layers or 3D objects helped by albedo and normal images
  (fine detail stays sharp); the export message names the device.
- Procedural noise in the formula language (`noise`, `perlin`, `voronoi`) and the colorings
  *Noise pattern* (value, Perlin, Voronoi or fractal noise; as lit heights a bump texture)
  and *Image along the bands*.
- Layer effects (inspector section *Layer effects*): drop shadow, outer glow, inner glow and
  neon edges by distance estimation, on the GPU and the CPU.

## [0.7.0] - 2026-11-27

### Added

- 3D object layers: Mandelbulb, quaternion Julia sets, Mandelbox and Menger sponge, sphere
  traced by their distance estimates from an orbiting camera (drag to orbit), lit with the
  lighting of lit layers (shadows, occlusion, reflections) and colored by an orbit trap; on
  the GPU and the CPU.
- Lit layers: any layer except density layers can be shaded as a relief (heights from the
  smooth iteration count, the coloring value or the brightness) with ambient light, diffuse
  light and highlights (GGX or Blinn-Phong) from up to four colored lights; on the GPU and
  the CPU. Brightness heights on image layers give bump maps. Soft shadows over the relief,
  ambient occlusion, reflections of a sky or a panorama image, heights from distance
  estimation. *3D terrain* shows the relief as a landscape under an orbiting perspective
  camera (height-field ray tracing over a maximum pyramid of the heights); Ctrl-drag orbits
  the camera, Ctrl-wheel changes its distance.

### Fixed

- The app could crash at start with "wgpu error: Out of Memory" when the interface scale
  was below 100 %: the first frames laid out the preview for a huge default screen. The
  preview is now never larger than the window, and a render too large for the GPU shows a
  message instead of crashing.

## [0.6.0] - 2026-11-27

### Added

- Density layers: orbits of the layer's formula from random start points, counted per pixel
  and tone-mapped (log density): Buddhabrot (escaping orbits), anti-Buddhabrot (captive
  orbits), Nebulabrot (three channels with their own iteration limits) and strange attractors,
  on the GPU and the CPU. New bundled family *Attractors* (Clifford, de Jong, Hopalong,
  Gumowski–Mira, Ikeda, Tinkerbell, Pickover, Lorenz, Rössler) and the gradients *Ember* and
  *Starlight*.
- L-system density layers: the turtle's strokes, antialiased, with presets and FractInt `.l`
  import.
- Kleinian density layers: limit sets of two-generator groups (Grandma's recipe from two
  traces, Riley's recipe; the Apollonian gasket among the presets).
- IFS density layers: the chaos game over affine maps with weights and variations (as in
  fractal flames), presets, view fitting, FractInt `.ifs` import.
- Formulas *Truchet tiles*, *Sierpinski carpet* and *Root finding (polynomial)* (Newton,
  Halley or Schröder steps for any polynomial up to degree 6, relaxation, Nova).
- Density layers: *Metropolis* sampling for zoomed Buddhabrots and anti-Buddhabrots (start
  points from Markov chains that favor orbits through the view, weighted so the density stays
  the same as with uniform start points).
- Formula language: `deriv(e, x)` gives the derivative of an expression with respect to a
  variable (also through variables assigned before), the same on the GPU and the CPU.
- Formula *Root finding (function)*: Newton, Halley, Schröder or Householder steps for
  f(z) = a·fn1(z)^p + b·fn2(z) + c, with derivatives from `deriv`.
- *Lyapunov* formula (Markus–Lyapunov sequences) with the *Lyapunov exponent* coloring;
  bundled formulas can suggest their coloring.
- Fill and image layers (layer menu): solid fill, gradient fill (new *Gradient fill*
  coloring: linear, radial, conical) and image layers.

## [0.5.0] - 2026-11-26

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
