# RFract

A 2D fractal generator in Rust: layered fractals in the spirit of Ultra Fractal and FractInt,
rendered on the GPU (wgpu), with denoising, post effects and animation.

Status: early development. Version 0.1 is the program shell and the release pipeline; the
renderer, layers, formulas and animation follow stage by stage (see [PLAN.md](PLAN.md)).

## Download

The first public release (1.0.0) follows once all stages of the plan are done. Until then,
development builds for Linux, Windows and macOS are attached as artifacts to the "Release"
workflow runs of the `v0.*` tags (Actions tab). The builds are not signed; see INSTALL.txt in
each package.

## Build

Rust 1.98.1 (pinned in `rust-toolchain.toml`):

```sh
cargo build --release -p rfr-app -p rfr-cli
python3 tools/fetch_oidn.py        # optional: denoiser libraries for development (oidn/)
./target/release/rfr-cli info
./target/release/rfr-app
```

## Related projects

- [RWildFire](https://github.com/Ravetracer/RWildFire) – fractal flames
- [RMandelbulb3D](https://github.com/Ravetracer/RMandelBulb) – 3D fractals

## License

GNU LGPL 3.0 or later (LICENSE, COPYING). Release packages include Intel Open Image Denoise
(Apache 2.0).
