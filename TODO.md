# TODO

The stages and their scope are in PLAN.md. This file holds the next concrete steps and wishes
that come up along the way.

## Next: stage 2 – layers, coloring, transformations, gradients

- [ ] Layer stack: groups, masks, merge modes, opacity, per-layer passes in the GPU renderer
- [ ] Coloring library (orbit traps, TIA, curvature, stripes, …)
- [ ] Transformations (pre/post)
- [ ] Gradient import/export (.map, .ugr, .ggr, .gpl, .cpt), separate alpha curve
- [ ] Live palette rotation in the viewer

## Open points from stage 1

- Distance estimation in double-float views falls back to the smooth iteration count (needs a
  dual double-float mode).
- UFM compatibility: 18 % of the entries in the local UFM database compile (string compares of
  enum parameters, classes, more `#` symbols are missing; stage 3).
- Integer arithmetic: the CPU uses 64-bit, the GPU 32-bit integers (only differs on overflow).
- The view's orbit display and a Julia preview inset are not there yet.

## Ideas

- Check Gnofract 4D's `formulas/` (BSD since 2.0) file by file and bundle what is clean.

- Shared crate with RWildFire / RMandelbulb3D after stage 2 (theme, OIDN, post, gradients).
