# RFract

A 2D fractal generator in Rust: layered fractals rendered on the GPU (wgpu), with denoising,
post effects and animation.

Status: development versions (0.x) on the way to 1.0. RFract has:

- Layers with merge modes, groups and masks; escape-time fractals (hundreds of built-in
  formulas), density layers (Buddhabrot, attractors, IFS, L-systems), Lyapunov, Kleinian
  limit sets, root finding, Truchet tiles.
- A formula language that reads `.ufm`, `.ucl`, `.uxf`, `.ulb` and `.frm` formula files and
  `.upr` and `.par` parameter files; a formula editor.
- Deep zoom to any depth (perturbation with linear approximation), automatic iteration
  counts, location bookmarks.
- Lighting and reliefs, 3D terrains and 3D objects, light sources with lens flares, effects
  (bloom, tone mapping, grading), the Intel Open Image Denoise denoiser.
- Animation (keyframes, flights, drivers, sound), video export through ffmpeg, an explore
  lane, posters beyond 20000 pixels, a render queue and a command line tool.

The stages and what comes next are in [PLAN.md](PLAN.md); the user manual is in
[docs/manual](docs/manual) (also inside the program, Help › Manual, and as a PDF with each
release).

## Download

Development versions are published as pre-releases on the
[releases page](https://github.com/Ravetracer/RFract/releases) for Linux (tar.gz, AppImage),
Windows (zip) and macOS (Apple silicon and Intel); the first full release (1.0.0) follows once
all stages of the plan are done. The builds are not signed; see INSTALL.txt in each package
and the notes on the releases page.

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
