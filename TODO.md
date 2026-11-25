# TODO

The stages and their scope are in PLAN.md. This file holds the next concrete steps and wishes
that come up along the way.

## Next: stage 3 – formula language compatibility (UFM and FractInt)

- [ ] UFM language: string comparisons of enum parameters, classes (UFM 5), more `#` symbols,
      color type and direct coloring (`#color`)
- [ ] `.upr` parameter files (layers, gradients, mappings)
- [ ] FractInt `.frm` front end, built-in types, `.par`, `.map`
- [ ] Formula editor and a browser for the user's formula folders
- Baseline (local UFM database): 18.2 % of the fractal formulas, 16.4 % of the colorings and
  34.7 % of the transformations compile.

## Open points from stage 2

- Coloring formulas cannot set colors directly yet (`#color`, needs the color type); histogram
  and Lyapunov colorings are missing (histogram needs a pass over the whole image).
- Post-transformations (on z after the iteration) are not there yet.

## Open points from stage 1

- Distance estimation in double-float views falls back to the smooth iteration count (needs a
  dual double-float mode).
- UFM compatibility: 18 % of the entries in the local UFM database compile (string compares of
  enum parameters, classes, more `#` symbols are missing; stage 3).
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
