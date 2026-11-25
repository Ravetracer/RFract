# TODO

The stages and their scope are in PLAN.md. This file holds the next concrete steps and wishes
that come up along the way.

## Next: stage 1 – escape-time core and viewer

- [ ] `.rfr` document model (layer-shaped, one layer used), serde JSON, version field
- [ ] Complex math helpers, view transform (center as decimal strings, log2 zoom, rotation)
- [ ] Minimal formula language: lexer, parser, type checker (UFM syntax subset)
- [ ] CPU closures back end, WGSL emitter
- [ ] Built-in formulas in `formula/lib/*.ufm`
- [ ] FRM-like v1 front end (spec: `_dev/fractalpark/docs/specs/frm-like-language-v1.md`); embed
      `crates/rfr-core/formulas/fractalpark` and compile all 534 for CPU and GPU
- [ ] Formula list in the layer panel; default views from `index.json` as presets
- [ ] Coloring: smooth iteration, distance estimation, solid / |z| inside
- [ ] Gradient: stops, OKLab interpolation, offset, density, transfer functions
- [ ] GPU renderer: progressive jittered AA, row bands, f32 + df64
- [ ] CPU reference renderer, CPU/GPU parity test
- [ ] Viewport navigation, Julia picker, undo/redo, save/load, PNG with embedded document
- [ ] `rfr-cli render`

## Ideas

- Check Gnofract 4D's `formulas/` (BSD since 2.0) file by file and bundle what is clean.

- Shared crate with RWildFire / RMandelbulb3D after stage 2 (theme, OIDN, post, gradients).
