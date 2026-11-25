# TODO

The stages and their scope are in PLAN.md. This file holds the next concrete steps and wishes
that come up along the way.

## Next: stage 3b – UFM classes

- [ ] `class`/`extends`, methods (virtual, static), fields, `import "x.ulb"`, `new`, object
      parameters (`param ... type`) and plugin parameters (`func`-like class choices).
- [ ] Nested plugin parameters in the document and the inspector.
- [ ] Own implementation of the standard base classes (formula, coloring, transform, gradient,
      generator); `common.ulb` from the user's formula folders.
- [ ] GPU through monomorphization (plugin choices are known when the shader is built).
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

- **Fly mode** (owner's wish, do whenever it fits): holding the left mouse button flies into
  the fractal toward the cursor continuously, the right button flies out, the mouse wheel sets
  the speed. The speed ramps up and down smoothly like a real drive (gentle acceleration
  when a button is pressed, gentle braking when it is released; XaoS does this nicely). Fast
  preview while flying, full quality once the flight stops; the cameraman/recording of
  stage 8 can reuse the flight path.

- Check Gnofract 4D's `formulas/` (BSD since 2.0) file by file and bundle what is clean.

- Shared crate with RWildFire / RMandelbulb3D after stage 2 (theme, OIDN, post, gradients).
