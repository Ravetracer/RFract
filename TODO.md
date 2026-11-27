# TODO

The stages and their scope are in PLAN.md. This file holds the next concrete steps and wishes
that come up along the way.

## Stage 9 – Explore, scale, polish

- [x] Explore lane: presets (the manual's examples), random documents framed by zooming into
      the busiest part of a neutral preview, mutations (parameters, colors, view, formula),
      quality filter (noise and contrast of the thumbnail), snapshots.
- [x] Tiled poster renders (20k+ px, tile-wise denoising): exact tiles for per-pixel layers,
      effects per region with margins, glow from a 4K render, PNG written in bands (20000 ×
      11250 in 76 s, 0.7 GB memory).
      Open: lit layers, layer effects, density layers and 3D objects in tiles (needs a
      "window of a larger frame" in both renderers).
- [x] Render queue polish: settings per job (size, samples, frames, output); posters above
      8K render in tiles as jobs.
- [x] Adaptive sampling: running mean per pixel, statistics (Σ√L, Σ L), done after 32
      samples when the pixel and its four neighbours are settled; layer kernels and the
      accumulation skip done pixels (CPU and GPU). 1080p, 256 spp: 19 s → 6 s, same error.
- [x] Shader cache on disk: not needed, the drivers cache compiled shaders (NVIDIA: 130–720 ms
      first, 20–80 ms after).
- [x] CPU guessing (solid guessing per 16-px tile on every pass): blocks whose border stayed
      inside with one color are filled; 1280×720 views with large inside areas 2.4–2.9×
      faster. Loses isolated slow-escaping pixels inside such blocks (seahorse example:
      270 of 921600 pixels at 4 spp); `rfr-cli --no-guess` turns it off.
- [x] Manual: how-tos chapter, introduction updated, screenshots of the timeline, explore lane
      and render queue; example documents as explore presets.
- [ ] Optional: network rendering (`rfr-net`), Lua scripting, shared crate extraction.

## Stage 8 – Animation

- [x] Core: keyframes as documents, JSON blend (monotone cubic, log fields, unwrapped angles),
      view path (log zoom, target in place, exact centers), timing per keyframe or steady,
      palette rotation, motion blur (sub-frames over the shutter).
- [x] Frame rendering (one renderer for all frames, reference orbits waited for), video
      through ffmpeg (H.264, H.265, AV1, ProRes, VP9, GIF) or PNG frames, `rfr-cli animate`.
- [x] Timeline in the app: keyframes from the current document, scrubbing, preview playback,
      animation export with progress.
- [x] Deep-zoom sequence mode: one image per 2× zoom step (at twice the frame size, kept in
      memory two at a time), frames composed by scaling; motion blur by blending zooms.
- [x] Fly mode (hold to fly in/out toward the cursor, smooth speed ramps), flight recording →
      keyframes (fewest within a tolerance), recorded timing with smoothed speed, cameraman
      smoothing (Gaussian, scale-aware centers, ends kept).
- [x] Drivers and expressions, audio-reactive values (sound under the movie), render queue
      (app window, `rfr-cli batch`).
- [x] Exit: 1-minute 1080p deep zoom with palette rotation and motion blur (app and CLI);
      fly-over of a lit Mandelbrot landscape.
      Note: the full render ran through the CLI (1800 frames to 1e-40, 52 min,
      `docs/manual/examples/deep-zoom.rfr`). The same render from the app (render queue) and
      the 1080p fly-over (`docs/manual/examples/flyover.rfr`) were only checked at small size
      and partial length; the full renders are still to be done (about 70 min).

## Stage 7 – Effects and textures

- [x] Global post: exposure, tone mapping (Reinhard, ACES, filmic, AgX), contrast,
      saturation, white balance, split toning, bloom pyramid, streaks, sharpen, lens
      distortion, chromatic aberration, vignette, grain; preview and export agree.
- [x] More post: lens flares and god rays (light position), curves and levels, spectral
      chromatic aberration.
- [x] Layer effects: DE-based edge glow (neon), drop shadow, outer and inner glow (emboss:
      lit layers with brightness heights).
- [x] Texture mapping: image textures through coloring coordinates (final z, pixel, orbit
      trap, iteration × angle), procedural patterns (value, Perlin, Voronoi, fractal noise),
      textures as bumps (lit layers with coloring heights).
- [x] Droste (log-polar spiral) as a transformation (the *Droste spiral* of stage 2).
- [x] Denoiser (OIDN) for exports and the command line (color only).
- [x] Denoiser: albedo and normal images from lit layers and objects (keeps fine detail).
- [x] Exit: effects available in preview and export with identical results (tested within 1/255).

## Stage 6 – 3D looks: lighting, 2.5D, 3D objects

- [x] Lit layers: a height field per pass (smooth iterations, coloring value, luminance),
      normals from neighboring heights, ambient + diffuse + specular (Blinn-Phong, GGX), up to
      four colored lights; CPU and GPU (field pre-pass like density layers). Luminance on image
      layers gives bump maps.
- [x] Shadows marched over the height field, ambient occlusion.
      Open: beyond the image border the heights continue the edge (a high edge casts shadows
      from outside); a field margin of the shadow length would fix that.
- [x] Environment reflections (gradient sky or image), distance estimation as height source.
- [x] 3D height view: the field as a terrain under a perspective camera (height-field
      ray tracing over a maximum pyramid, CPU and GPU).
- [x] Terrain: orbit (Ctrl-drag) and dolly (Ctrl-wheel) the camera with the mouse.
- [ ] Terrain: larger than the view (more detail near the camera), optional walls down to a
      base, optional path tracing into OIDN.
- [x] 3D object layers: Mandelbulb, quaternion Julia, Mandelbox, Menger sponge, sphere traced
      with the same lights (shadows, occlusion, reflections).
- [ ] 3D objects: bicomplex Julia and more DE formulas, formulas from the formula language
      (3D programs), depth for compositing with terrains.
- [x] Exit: lit 2.5D renders, 3D terrains and objects (the fly-over moved to stage 8).

## Stage 5 – More fractal types (layer kinds)

- [x] Density layers (CPU and GPU): Buddhabrot, anti-Buddhabrot, Nebulabrot, strange
      attractors (bundled family), log-density tone mapping, restart while the counts fill.
- [x] Lyapunov (Markus–Lyapunov sequences) with its coloring.
- [x] Fill and image layers (presets of the Pixel formula), gradient fill coloring.
- [x] IFS density layers (affine maps, ten variations, presets, view fitting) and `.ifs` import.
- [x] Root finding for polynomials (Newton, Halley, Schröder, relaxation, Nova).
- [x] Root finding for any formula with derivatives from the compiler (`deriv(e, x)`).
- [x] Kleinian limit sets (Grandma's recipe, Riley; Maskit and the Apollonian gasket as cases).
- [x] Truchet tiles and the Sierpinski carpet.
- [x] L-systems (density layer of strokes, presets, `.l` import).
- [x] Density layers: Metropolis–Hastings sampling for zoomed Buddhabrots.

## Stage 4 – Deep zoom

- [x] Views at any depth (floatexp radius, decimal center, exact panning and zooming).
- [x] Perturbation derived from the formula (sums, products, quotients, integer powers, conj,
      abs via diffabs, real/imag/flip), bignum reference orbit (dashu), floatexp deltas with
      rebasing; CPU and GPU (`Mode::Pert`), parity tests.
- [x] BLA (linear steps derived from the formula, binary table, floatexp matrices).
- [x] Reference orbit kept while navigating near it, new ones in the background.
- [ ] Periodicity checking and interior checks (cardioid/bulbs) for faster inside regions.
- [ ] Automatic iteration count (raise while pixels at the limit border escaping ones).
- [ ] Location bookmarks.
- [ ] Distance estimation in perturbation views (derivative deltas), BLA for formulas that
      carry more than z, reference values in double-float on the GPU (long orbits).
- [ ] Formulas that cannot be perturbed: CPU arbitrary-precision fallback (slow) or a hint.

## Stage 3b – UFM classes (done, v0.4.1)

- Classes, inheritance, methods, static and base calls, fields, object parameters (plugins)
  with nested parameters, casts, compiled into plain IR (GPU and CPU); arrays of objects,
  counting loops unrolled (also up to a parameter's maximum), objects chosen at run time
  (tags), image parameters and the *Image* coloring, arithmetic on colors, assignments inside
  expressions, formula functions, array copies, RFract's standard classes on top of
  `common.ulb` (formulas, colorings, orbit traps, trap shapes, transfers).
- Local database (default plugins): 97.9 % of all entries compile and render (was 97.3 %);
  class colorings 65 of 80 (was 35), class fractal formulas 43 of 48 (was 36); classic
  fractal formulas 98.9 %, colorings 90.4 %, transformations 97.9 %.
- Open: `#skew`, `#calculationpurpose`, `srand`, image-sized arrays (whole-image
  accumulation), class libraries that are not in the database (`JLB_SwitchCombo`, …), the
  ray tracers' huge object lists, loops that change which object a variable refers to.
- [ ] Optional: Cranelift JIT for the CPU path.

## Done in stage 3 (v0.4.0)

- UFM: arrays, color type and direct coloring, FractInt syntax, preprocessor; local database:
  classic fractal formulas 98.4 %, colorings 89.1 %, transformations 97.1 % compile and render.
- FractInt: `.frm` files 98.6 % (orgform), 40 escape-time types, `.par` with palettes.
- Formula folders, `.upr` import (all 1571 images open, 51.6 % without notes), formula editor.
- Open: image-sized arrays (Buddhabrot-style accumulation over the whole image), assignments
  inside expressions (FractInt), orbit/attractor/IFS/cellular FractInt types, color function
  parameters (`color func` with merge modes), UFM user functions.

## Open points from stage 2

- Histogram coloring is missing (needs a pass over the whole image).
- Post-transformations (on z after the iteration) are not there yet.

## Open points from stage 1

- Distance estimation in double-float views falls back to the smooth iteration count (needs a
  dual double-float mode).
- Integer arithmetic: the CPU uses 64-bit, the GPU 32-bit integers (only differs on overflow).
- The view's orbit display and a Julia preview inset are not there yet.

## Ideas

- Check Gnofract 4D's `formulas/` (BSD since 2.0) file by file and bundle what is clean.

- Shared crate with RWildFire / RMandelbulb3D after stage 2 (theme, OIDN, post, gradients).
