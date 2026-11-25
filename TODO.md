# TODO

The stages and their scope are in PLAN.md. This file holds the next concrete steps and wishes
that come up along the way.

## Next: stage 1 – escape-time core and viewer

- [ ] `.rfr` document model (layer-shaped, one layer used), serde JSON, version field
- [ ] Complex math helpers, view transform (center as decimal strings, log2 zoom, rotation)
- [ ] Minimal formula language: lexer, parser, type checker (UF syntax subset)
- [ ] CPU closures back end, WGSL emitter
- [ ] Built-in formulas in `formula/lib/*.rff`
- [ ] Coloring: smooth iteration, distance estimation, solid / |z| inside
- [ ] Gradient: stops, OKLab interpolation, offset, density, transfer functions
- [ ] GPU renderer: progressive jittered AA, row bands, f32 + df64
- [ ] CPU reference renderer, CPU/GPU parity test
- [ ] Viewport navigation, Julia picker, undo/redo, save/load, PNG with embedded document
- [ ] `rfr-cli render`

## Ideas

- Shared crate with RWildFire / RMandelbulb3D after stage 2 (theme, OIDN, post, gradients).
