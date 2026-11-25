# RFract – project plan

RFract is a 2D fractal generator in Rust: layered escape-time fractals in the spirit of Ultra
Fractal, plus density, IFS and 2.5D modes, GPU rendering (wgpu), OIDN denoising, animation and
post effects. It is the third sibling of RWildFire (flames) and RMandelbulb3D (3D) and shares
their stack, UI conventions and release tooling.

Status: stage 0 (foundation and distribution) done; stage 1 next.

---

## 1. Goals and non-goals

**Goals**

- Layered fractal images: any number of layers, each with its own formula, transformations,
  inside/outside coloring, gradient, merge mode, opacity and mask (the Ultra Fractal model).
- Read the Ultra Fractal formula language (`.ufm`, `.ucl`, `.uxf`, `.ulb`) and parameter files
  (`.upr`), FractInt formulas (`.frm`), parameters (`.par`) and palettes (`.map`).
- GPU first (wgpu compute, WGSL generated per layer), with a CPU reference renderer of the same
  algorithm (Cranelift JIT) for tests, fallback and features the GPU cannot do.
- Deep zoom far beyond double precision (perturbation, BLA, extended exponent).
- Animation with keyframes, camera smoothing ("cameraman"), motion blur, palette rotation,
  drivers and audio-reactive values, video export.
- Effects: glow/bloom, streaks, chromatic aberration, lens flares, lighting of the fractal as a
  height field, texture mapping, tone mapping.
- Same look and working style as RMandelbulb3D (egui theme, panels, hover text on every control,
  embedded manual, render window, explore lane, render queue).
- Packages for Linux, Windows and macOS (unsigned) from GitHub Actions on every tag.

**Non-goals**

- Full fractal-flame editing: that is RWildFire. RFract gets a light IFS layer only (see stage 5).
- Full 3D scenes: that is RMandelBulb3D. RFract gets 2.5D height fields and a small set of 3D
  object layers that composite into the 2D stack (stage 6).
- Exporting back to Ultra Fractal or FractInt formats.
- Bundling third-party formula collections (licensing, see section 8).

---

## 2. Reference material

| Source | Use |
|--------|-----|
| `_dev/Ultra Fractal - Formulas` | Public UF formula database: ~1140 coloring, ~480 transformation and several thousand fractal entries in 379 files; 328 files use no class imports (UF4 style). `common.ulb` is present; UF's built-in `Standard.ulb` is not public and must be re-created. Test corpus for the language compiler, never committed. |
| `_dev/frasrc-20.04p14` | FractInt 20.4 source ("copyrighted freeware"): reference for built-in fractal types, `.frm`/`.par`/`.map`/`.ifs`/`.l` formats, guessing/boundary tracing. Read for algorithms, do not copy code. Never committed. |
| `crates/rfr-core/formulas/fractalpark` | **Bundled.** FractalPark's published formula library (534 Definitions in FRM-like v1, MIT, after FractalPark's own rights review: 339 independent rewrites, 106 adaptations, 89 own). Names, families, parameters and a default view per formula in `index.json`. Updated by `tools/import_fractalpark.py`; never copy its unpublished candidate folders or held GPL rows. |
| `_dev/fractalpark` | FractalPark checkout: the FRM-like v1 spec (`docs/specs/frm-like-language-v1.md`, `docs/manuals/frm-like-v1.md`), stdlib semantics and its renderer as a behavior reference. |
| `_dev/fract4d-formulas` | fract4d/formulas: the repository is MIT, but every formula file is in `orgform/`, which its README excludes from the MIT license (FractInt collection ORGFORM.ZIP, copyright unclear). **Not bundled**: test corpus for the FractInt front end, user import only. |
| Gnofract 4D `formulas/` (github.com/fract4d/gnofract4d) | Program is BSD-licensed since 2.0 (GPL before). `gf4d.frm`, `gf4d.cfrm`, `gf4d.uxf`, `standard.ucl`, `fractint-builtin.frm` (FractInt built-in types written as formulas) are bundling candidates after a per-file header check. |
| ultrafractal.com features / formula reference | Language semantics, layer model, merge modes, gradient model, coloring concepts. |
| Xenodream / JUX | Shaded IFS, depth lighting, texture ideas. |
| Chaotica, Fractorium, Apophysis | Density rendering, tone mapping (already covered by RWildFire). |
| FRAX, Mandelbrowser, IceFractal | Deep zoom UX, 2.5D/3D height views, palettes, mobile-grade performance tricks. |
| `../RWildFire`, `../MandelBulb/RMandelbulb3D` | Code to reuse (section 4). |

---

## 3. Architecture

### 3.1 Workspace

Same shape as RMandelbulb3D (Rust 2024, `resolver = "3"`, toolchain pinned in
`rust-toolchain.toml`, one workspace version):

| Crate | Content |
|-------|---------|
| `rfr-core` | Document model, complex/bignum math, formula language (parser, type checker, CPU closures, Cranelift JIT, WGSL emitter), built-in formula library (`formula/lib/*.rff`), coloring and transformation library, gradients and palettes, CPU reference renderer, compositing, post processing, denoiser glue, animation, video, IO (`.rfr`, PNG with parameters, UF/FractInt import) |
| `rfr-gpu` | wgpu renderer: per-layer generated WGSL, composite and accumulation passes, `df64.wgsl`, perturbation kernels, density kernels, post/display shaders, batch/tiled rendering |
| `rfr-app` | egui/eframe application |
| `rfr-cli` | Command line: render, animate, batch, formula check/import report, denoise |
| `rfr-net` | (stage 9, optional) network rendering, port of `rmb-net` |

### 3.2 Document model

```
Document
├─ image: size, supersampling, background, color space, post settings
├─ view (per document, layers may override): center (decimal strings), zoom (log2),
│  rotation, skew/stretch
├─ layers: [Layer | Group]
│   Layer
│   ├─ kind: EscapeTime | Density | Ifs | LSystem | Image | Fill | HeightView3D | Object3D
│   ├─ formula ref + parameters      (escape-time: init/loop/bailout)
│   ├─ transformations: [pre], [post]
│   ├─ outside: coloring + gradient + transfer function + density/offset
│   ├─ inside:  coloring + gradient + transfer function + density/offset
│   ├─ merge mode, opacity, visible, locked
│   ├─ mask: none | layer ref | own mask stack (invert, feather)
│   └─ layer effects (stage 7): glow, drop shadow, emboss, lighting
├─ lights (stage 6), textures/images
├─ animation: keyframes, drivers, audio, shutter
└─ embedded formula texts (non-built-in formulas the document uses)
```

- File format `.rfr`: versioned JSON. PNG export embeds the document (tEXt chunk) so every
  image reopens as its parameters (as in RMandelbulb3D).
- Coordinates: view center as decimal strings plus a cached f64; zoom as log2 so animation can
  interpolate it linearly.

### 3.3 Formula language and compilers

- One AST for all sources. Front ends: RFract's own syntax (= the Ultra Fractal language, so
  UF formulas are native, not "converted"), FractInt `.frm` (small, maps onto the same AST),
  FractalPark FRM-like v1 (`frm-like/1`: typed, normative spec, the bundled library), and the
  Gnofract 4D dialect (`.frm`/`.cfrm`/`.uxf`, close to UF) for user imports.
- Back ends, as in RMandelbulb3D: CPU closures, Cranelift JIT over the same register file
  (bit-identical helper calls for transcendentals), WGSL emitter (f32, df64, perturbation
  variant).
- **One formula source**: built-in formulas exist once, as language text in
  `formula/lib/*.rff`. Never hand-write a formula twice for CPU and GPU.
- Parameters, iteration counts, gradients and all render settings live in buffers, so editing
  never recompiles. The shader signature is the hash of the formula/coloring/transform texts,
  merge modes and the precision variant of a layer. A fully baked shader (constants folded) is
  compiled in the background once the view rests.
- UF5 classes: compiled by monomorphization (a class parameter is a compile-time choice, so
  virtual calls become static calls on the GPU). Dynamic arrays get a fixed upper bound on the
  GPU or force the CPU path for that layer, reported in the UI.
- Automatic differentiation in the compiler (dual numbers) gives derivatives for distance
  estimation, Newton/Nova roots and slope lighting without the formula author writing them.
- Automatic perturbation: for polynomial formulas (and `abs` via the "diffabs" rule) the
  compiler derives the delta iteration from the AST, so deep zoom works for Mandelbrot powers,
  Burning Ship, Tricorn, Celtic, Buffalo, … without hand-written perturbation code.

### 3.4 Rendering pipeline

Per frame / sample batch:

1. **Layer pass** (one compute pass per layer, small shader per layer, cached by signature):
   iterate the formula with pre-transforms, run inside/outside coloring, look up the gradient,
   write linear RGBA plus AOVs (albedo, normal from slope, height, smooth iteration, DE) into
   the sample buffer — blending onto the layers below with the layer's merge mode and mask.
   Same pixel, same thread: no races, no per-layer full-size buffers.
2. **Mask passes** before the layers that use them (one extra alpha buffer per mask group).
3. **Accumulate**: jittered sub-pixel positions (R2 low-discrepancy sequence, per-pixel offset,
   identical on CPU and GPU), running sum of color and AOVs.
4. **Develop** (display shader / CPU `post_process`): denoise (optional), bloom, chromatic
   aberration, flares, tone mapping, color grading. The raw accumulation is kept, so post
   changes never re-render.

- Row bands with adaptive height (~12 ms per submission): no driver timeouts, responsive UI.
- Density layers (Buddhabrot, attractors, IFS) scatter with atomics into a histogram and are
  tone-mapped (log density, gamma, vibrancy) before they enter the composite.
- Histogram coloring needs a first pass over iteration counts; done at low resolution, then
  applied as a LUT.
- Precision ladder per layer, automatic by zoom: f32 → df64 (or native f64 via
  `SHADER_F64` where available, not on Metal) → perturbation with floatexp → CPU arbitrary
  precision (any formula, slow).
- CPU renderer = same algorithm step by step, including random number order. Tests compare GPU
  and CPU renders (mean difference threshold, as `rmb-gpu/tests/shaders.rs`).

### 3.5 Denoiser

OIDN, runtime-loaded (from RWildFire/RMandelbulb3D: `render/oidn.rs`, `render/denoise.rs`),
fed with color + albedo + normal. Honest scope: escape-time images are clean once supersampled,
so OIDN matters mainly for density layers, path-traced 2.5D/3D views, glow/steam noise and
early progressive previews. Supersampling and adaptive anti-aliasing (more samples where the
variance is high) are the main quality tools for escape-time layers.

### 3.6 UI (adapted from RMandelbulb3D)

| Area | Content |
|------|---------|
| Top bar | File, Edit, View, Render, Animation, Help; precision/zoom depth indicator; GPU/CPU status |
| Left | Layer stack (groups, drag to reorder, eye/lock, merge mode, opacity, thumbnails), formula browser (search over built-in and imported collections, tags, cached thumbnails, favorites) |
| Center | Viewport: drag pan, wheel zoom around the cursor, box zoom, rotate (Ctrl), zoom history, Julia preview inset (Mandelbrot ↔ Julia switch by click), orbit display for the point under the cursor, guides, live palette rotation |
| Right | Inspector tabs: Formula, Transformations, Outside, Inside, Gradient, Mask, Layer effects, Lights, Image/Post |
| Bottom | Timeline (keyframes, recording, playback) and explore lane (presets, random, mutations, snapshots) |
| Windows | Render (progressive test render, denoise toggle, post editing on the raw image), Export (image, animation, queue), Gradient editor, Formula editor (syntax highlight, error markers, live compile, CPU/GPU support report), Queue, Manual |

Owner rules carried over: every control has a hover text; every action reports what it did or
why it was skipped; undo/redo for all document changes; manual updated with each feature.

---

## 4. Reuse from the sibling projects

Copy and adapt (all are the owner's own Rust code; keep license compatibility, section 8):

| From RMandelbulb3D | Purpose in RFract |
|--------------------|-------------------|
| `rmb-app/theme.rs`, `widgets.rs`, `icons.rs`, `prefs.rs`, `doc.rs` (undo), `help.rs`/`manual.rs`, `render_view.rs`, `export.rs`, `queue_window.rs`, `timeline.rs`, `palette_editor.rs`, `explorer.rs` | UI shell, same look |
| `rmb-core/render/oidn.rs`, `denoise.rs`, `bloom.rs`, `post_process`, chromatic aberration, `flare.rs` | Denoiser and post effects |
| `palette.rs` (OKLCh generator), `gradient.rs` (LUT, glow bands) | Gradients, palette generator |
| `animation.rs` (keyframes, cameraman smoothing, flight → keyframes), `expr.rs`, `audio.rs`, `video.rs`, `queue.rs`, `paths.rs` | Animation, drivers, video, queue, platform folders |
| `formula/lang.rs`, `cpu.rs`, `jit.rs`, `wgsl.rs` (architecture, not the MB3D-specific language) | Compiler structure |
| `rmb-gpu/df64.wgsl`, `renderer.rs` (bands, async compile, baking), `bloom.wgsl`, `display.wgsl` | GPU infrastructure |
| `tools/release.py`, `package.py`, `fetch_oidn.py`, `build_manual.py`, `packaging/`, `.github/workflows/*` | Releases and packaging |

| From RWildFire | Purpose |
|----------------|---------|
| Density histogram + tone mapping, `gradient_io.rs` (gradient formats), `curve_editor.rs` | Density layers, gradient import, transfer curves |

Later option: once three projects carry the same copies, extract a shared crate
(`fractal-common`: theme, OIDN, post, gradients, animation, video, packaging tools). Not before
RFract stage 2 – the APIs should settle first.

---

## 5. Stages

Each stage ends with a tag built by the pipeline from stage 0, a manual update and a CHANGELOG
entry. Versions during development: stage N ends with `v0.N.0` (`v0.1.0` = stage 0, …,
`v0.10.0` = stage 9); `v0.*` tags build all packages as workflow artifacts but publish no
GitHub release. The first public release is **1.0.0**, after all stages. Sizes are relative (S < M < L < XL).

### Stage 0 – Foundation and distribution (S)

- Workspace with the five crates (net later), `rust-toolchain.toml` (same pin as RMandelbulb3D),
  `AGENT.md`/`CLAUDE.md`, `CHANGELOG.md`, `LICENSE`, `NOTICE`, `TODO.md`.
- `.gitignore`: `target/`, `dist/`, `oidn/`, `_dev/`, `docs/manual/build/`.
- `tools/`: `release.py` (bump, changelog, tag), `package.py` (linux tar.gz, AppImage, windows
  zip, macOS `.app` zip per arch), `fetch_oidn.py`, `build_manual.py`.
- `packaging/`: `.desktop` + `AppRun`, `Info.plist`, icon set (`.ico`, `.icns`, PNG), INSTALL
  texts per platform (macOS: Gatekeeper / `xattr -dr com.apple.quarantine`; Windows:
  SmartScreen "More info → Run anyway"), release footer.
- `.github/workflows/ci.yml`: clippy `-D warnings` + tests without GPU on push/PR
  (ubuntu-22.04).
- `.github/workflows/release.yml` on tag `v*` (and manual dispatch → artifacts only):
  - Linux x86_64 on ubuntu-22.04 (glibc 2.35): tar.gz + AppImage
  - Windows x86_64 (MSVC): zip with exe, OIDN DLLs, manual
  - macOS arm64 and x86_64 (`MACOSX_DEPLOYMENT_TARGET=11.0`): `.app` with ad-hoc signature,
    OIDN in `Contents/Frameworks`, zipped
  - release job: notes from the CHANGELOG section, `gh release create --verify-tag`
- egui shell: theme, empty panels in the final layout, prefs, about box, Windows exe icon
  (`winresource`).
- **Exit**: tag `v0.1.0` publishes five packages that start on all three platforms.

### Stage 1 – Escape-time core and viewer (L)

- Complex math, `.rfr` model (already layer-shaped, one layer used).
- Minimal formula language (expressions, `init`/`loop`/`bailout`, params of type
  float/int/complex/bool/enum) with CPU closures and WGSL emitter. Built-ins written in it:
  Mandelbrot (z^n, real powers), Julia, Burning Ship, Tricorn/Mandelbar, Celtic, Buffalo,
  Perpendicular, Phoenix, Lambda, Magnet I/II, Newton, Nova, Spider, Manowar, Barnsley 1–3.
- Coloring: smooth iteration (continuous potential), distance estimation, solid inside,
  |z| inside.
- Gradient: stops, linear/smooth interpolation in OKLab, offset, density, transfer functions
  (linear, sqrt, cube root, log, exp); basic gradient editor (from RMandelbulb3D).
- GPU renderer: progressive jittered AA, bands, f32 and df64; CPU reference renderer; CPU/GPU
  parity test.
- Viewer navigation (pan, zoom, box zoom, rotate, history), Julia picker, undo/redo,
  save/load, PNG export with embedded document, `rfr-cli render`.
- FractalPark FRM-like v1 front end: the 534 bundled Definitions become the formula library
  of stage 1 (their default views give presets and browser thumbnails). They are a typed,
  specified, normalized corpus – the best test bed for the compiler and the CPU/GPU parity
  test from day one. Simple formula list in the layer panel (search by name and family).
- **Exit**: smooth 1080p exploration of Mandelbrot/Julia families down to 1e-13, parity test
  green, all 534 bundled FractalPark formulas compile for CPU and GPU (report of exceptions).

### Stage 2 – Layers, coloring, transformations, gradients (L)

- Layer stack with groups, masks (layer as mask, invert, feather), opacity, visibility, lock.
- Merge modes: normal, multiply, screen, overlay, soft light, hard light, color dodge/burn,
  darken, lighten, difference, exclusion, addition, subtraction, hue, saturation, color,
  luminance, alpha only (UF set plus common extras).
- Separate inside/outside coloring with own gradient and alpha; direct (RGBA) coloring.
- Coloring library: orbit traps (point, line, circle, cross, spiral, astroid, heart, rectangle,
  image trap), triangle inequality average, curvature average, stripe average, exponential
  smoothing, binary decomposition, field lines, angle/decomposition, Gaussian integer,
  Pickover stalks, epsilon cross, bof60/61, Lyapunov, histogram coloring.
- Transformations (pre/post): affine, mirror, kaleidoscope, polar/inverse, Möbius, Riemann
  sphere, tiling/wallpaper groups, twirl/ripple, Droste, Julia/Mandelbrot switch.
- Gradient editor complete: per-channel curves including a separate alpha curve, generator and
  harmonies (OKLCh, from RMandelbulb3D), randomize, whole-gradient edits, library browser,
  import `.map`, `.ugr`, `.ggr`, `.gpl`, `.cpt`, `.grd` (Photoshop, basic), `.flame` palettes;
  export `.ugr`/`.map`/`.ggr`.
- Live palette rotation (color cycling) in the viewer.
- **Exit**: multi-layer images in the style of the UF showcase can be built from built-ins only.

### Stage 3 – Formula language compatibility: Ultra Fractal and FractInt (XL)

- **3a – classic UF language**: all sections (`global`, `init`, `loop`, `bailout`, `final`,
  `transform`, `default`, `switch`), all types (bool, int, float, complex, color, hyper),
  `@fn` function params, all param kinds (enum, func, color, image, heading), `#`-symbols
  (`#pixel`, `#z`, `#numiter`, `#maxiter`, `#solid`, `#index`, `#color`, `#screenpixel`, …),
  built-in functions and color functions (`rgba`, `blend`, `compose`, `hsl`, …), user
  functions, `while`/`repeat`, static arrays. Target: the 328 import-free files.
- **3b – UF5 classes**: `class`/`extends`/virtual methods, `import`, `new @param_class`,
  dynamic arrays; own implementation of the `Standard.ulb` base classes (formula, coloring,
  transform, gradient, generator interfaces) from the public formula reference; `common.ulb`
  from the user's folder. GPU via monomorphization, CPU fallback where needed.
- Cranelift JIT for the CPU path; WGSL emitter for everything the GPU supports; per-layer
  report of what forced the CPU path.
- `.upr` import (UF parameter sets: formulas by `file:entry`, layers, gradients, mappings).
- Compatibility report: `rfr-cli formulas <folder>` compiles every entry, renders a thumbnail,
  prints counts and failure reasons (like RMandelbulb3D's `m3f` report). Run locally over
  `_dev`; CI uses a small set of self-written test formulas.
- FractInt: `.frm` front end (tested against `_dev/fract4d-formulas/orgform`, 321 files), the
  most used built-in types (Mandel/Julia variants, lambda,
  magnet, popcorn, Newton/complex Newton, Barnsley, Halley, Phoenix, Frothy basin, Lyapunov,
  Volterra–Lotka, quaternion/hypercomplex slices, …), `.par` import, `.map` palettes.
- Formula editor and formula browser in the app; the user points RFract to their UF formula
  folder (auto-detect the default UF folder) and FractInt folders.
- **Exit**: ≥ 90 % of the classic UF entries in `_dev` compile and render, measured by the
  report; selected `.upr` files from the UF showcase render recognizably.

### Stage 4 – Deep zoom (L)

- Reference orbit in arbitrary precision with a pure-Rust bignum (`astro-float` or `dashu`; no
  GMP, so Windows/macOS CI stays simple).
- Perturbation on the GPU with floatexp (mantissa + extended exponent) below 1e-300,
  bilinear approximation (BLA) to skip iterations, glitch detection and rebasing, additional
  references when needed.
- Compiler-derived perturbation for polynomial formulas (section 3.3); other formulas: df64 /
  CPU arbitrary precision.
- Periodicity checking, interior checks (cardioid/bulbs), auto iteration count.
- Location bookmarks, "zoom to depth", depth display (10^-N), precision indicator.
- **Exit**: Mandelbrot and Burning Ship at 1e-1000 interactive (seconds per frame at 1080p
  preview), CPU/GPU parity for perturbation within tolerance.

### Stage 5 – More fractal types (layer kinds) (L)

- Density: Buddhabrot, anti-Buddhabrot, Nebulabrot (RGB channels by iteration ranges), with
  Metropolis–Hastings sampling for zooms; strange attractors (Clifford, de Jong, Hopalong,
  Gumowski–Mira, Ikeda, Tinkerbell, Pickover, Lorenz/Rössler projections); log-density tone
  mapping (from RWildFire).
- Light IFS layer: affine maps plus a small variation set, shaded by depth/orbit like JUX;
  `.ifs` import. Heavy flame work stays in RWildFire (optional later: render an RWildFire
  `.flame` into a layer through `rwf-core` as a dependency).
- Lyapunov (Markus–Lyapunov sequences), generic root finding (Newton, Halley, Householder,
  Nova) with compiler-derived derivatives, Kleinian/limit sets (Maskit, Riley, Grandma's
  recipe), Apollonian/inversion fractals, Truchet and tiling fractals, L-systems (`.l`
  import, vector layer, antialiased strokes).
- Image layer (bitmap for compositing, masks and traps) and fill layer (solid/gradient).
- **Exit**: each kind available as a layer, composited with escape-time layers.

### Stage 6 – 3D looks: lighting, 2.5D, 3D objects (L)

- Height field from any layer value (smooth iteration, DE, potential, luminance): normals,
  Blinn-Phong/GGX lighting, several colored lights, shadows marched over the height field,
  ambient occlusion, environment reflections, bump and normal maps from textures (UF "slope"
  colorings, done properly).
- 3D height view: the field as a terrain under a perspective camera (GPU height-field
  raymarching), optional path tracing → OIDN.
- 3D object layers composited into the stack: quaternion/bicomplex Julia, Mandelbulb and a
  few DE formulas, raymarched with lighting (small subset of RMandelBulb3D ideas, not a port).
  Optional later: render an RMandelBulb3D `.rmb` scene into a layer.
- **Exit**: lit 2.5D renders and a fly-over animation of a Mandelbrot landscape.

### Stage 7 – Effects and textures (M)

- Global post (from RMandelbulb3D): bloom/glow pyramid, streaks, chromatic aberration (lateral,
  radial, spectral), lens flares, vignette, film grain, tone mapping (ACES, AgX, filmic,
  Reinhard), white balance, split toning, curves/levels, sharpen, lens distortion, god rays
  from light positions.
- Layer effects: DE-based edge glow/neon, drop shadow, emboss, outer/inner glow.
- Texture mapping: image textures through coloring UVs (orbit-trap coordinates, iteration ×
  angle, Droste), procedural patterns (Perlin, Voronoi, value noise), textures as bump/normal.
- Denoiser wiring for all layer kinds (AOVs), GPU OIDN where available.
- **Exit**: effects available in preview and export with identical results.

### Stage 8 – Animation (L)

- Keyframe timeline (from RMandelbulb3D): every parameter animatable, including layer
  opacity, gradient stops, transforms, lights; interpolation modes (linear, Catmull-Rom, TCB,
  ease); zoom interpolated in log space at constant perceived speed; rotation unwrapping.
- Cameraman smoothing, flight recording → keyframes, palette rotation over time, drivers and
  expressions, audio-reactive values (bass/mid/treble).
- Motion blur: shutter angle, sub-frame samples per pass.
- Export: PNG/EXR sequences, video through ffmpeg (H.264, H.265, ProRes, AV1, GIF); render
  queue; low-resolution preview playback.
- Deep-zoom sequence mode: render one EXR per 2× zoom step and compose the frames by scaling
  (orders of magnitude faster for long zoom videos).
- **Exit**: a 1-minute 1080p deep zoom with palette rotation and motion blur exported from the
  app and the CLI.

### Stage 9 – Explore, scale, polish (M)

- Explore lane: presets, random documents, mutations, quality filter (from RMandelbulb3D).
- Tiled poster renders (20k+ px, tile-wise denoising), batch rendering, render queue polish.
- Optional: network rendering (`rfr-net`, port of `rmb-net`), Lua scripting (from RWildFire),
  shared crate extraction (section 4).
- Performance: adaptive AA, CPU guessing/boundary tracing, shader cache on disk.
- Complete manual with how-tos and example documents.

---

## 6. Cross-cutting rules

- CPU and GPU implement the same algorithm step by step; change both together; parity tests.
- One formula source (language text), never duplicated per back end.
- Editing parameters never recompiles shaders.
- Hover text on every control; actions report outcome or reason for skipping.
- Every feature updates `docs/manual` and `CHANGELOG.md` (`## [Unreleased]`).
- Versioning `MAJOR.FEATURE.FIX`, one version for all crates, releases only through
  `tools/release.py` + tag.
- Half the CPU cores for builds and tests (`-j 8`, `RAYON_NUM_THREADS=8`).

---

## 7. Testing

| Test | Where |
|------|-------|
| Language: parser/type checker unit tests, JIT vs closures on random points for every built-in | CI |
| WGSL validation of every built-in formula/coloring/transform with naga | CI |
| GPU vs CPU render comparison, df64 and perturbation accuracy vs f64/bignum | local (needs GPU) |
| UF/FractInt compatibility report over `_dev` | local |
| Golden images for built-in presets (small size, tolerance) | CI (CPU renderer) |
| Packaging smoke test: package job starts the CLI (`--version`, tiny CPU render) | release workflow |

---

## 8. Licensing and distribution notes

- RFract license: **LGPL-3.0-or-later** (compatible with code copied from RWildFire,
  LGPL-3.0+, and RMandelbulb3D, LGPL-2.1+ which may be used under 3.0).
- Bundling policy: only formula sets with a clear license, kept in their own folder with the
  license text and a SOURCE.md (repository, commit, counts), credited in NOTICE,
  THIRD_PARTY_NOTICES.md (shipped in every package) and Help › About. Every formula keeps its
  origin (set, source revision) in the library so the browser can show it.
  - Bundled: FractalPark published set (MIT).
  - Candidate: Gnofract 4D formulas (BSD, check each file header first).
  - Not bundled: fract4d/formulas `orgform/` (excluded from its MIT license, rights unclear),
    UF formula database, FractInt files – user imports and local test corpora only.
- UF formula files belong to their authors: not bundled; RFract imports them from the user's
  folder.
- FractInt is copyrighted freeware: formats and algorithms are re-implemented, no code copied,
  no files bundled.
- "Ultra Fractal", "FractInt" only in descriptive use ("Import Ultra Fractal formulas").
- OIDN (Apache-2.0) bundled per platform with its license; ffmpeg not bundled (detected at run
  time, path configurable).
- Unsigned packages: macOS `.app` with ad-hoc signature (Gatekeeper note in INSTALL text),
  Windows exe without Authenticode (SmartScreen note).

---

## 9. Open decisions

Decided: LGPL-3.0-or-later; repository `github.com/Ravetracer/RFract`.

1. Own formula file extension for RFract's own built-ins (`.rff` proposed) vs. using `.ufm`
   directly.
2. Light IFS layer only, or a real `.flame` layer through `rwf-core`.
3. Whether to extract a shared crate with the siblings now or after stage 2 (suggested: after).
