# AGENT.md – working on RFract

Notes for coding agents (and humans). RFract is a 2D fractal generator in Rust (layered fractals,
GPU rendering, denoiser, animation). The staged plan is `PLAN.md`;
next concrete steps are in `TODO.md`. Siblings with code to reuse: `../RWildFire` (flames) and
`../MandelBulb/RMandelbulb3D` (3D); `PLAN.md` section 4 lists what comes from where.

## Owner and working style

- Keep answers short. Report outcomes honestly (failed tests, skipped steps, limitations).
- **Use half of the CPU cores**: `cargo build -j 8`, `cargo test -j 8 -- --test-threads=8`,
  `RAYON_NUM_THREADS=8`.
- At most two sub-agents at a time, and tell them not to spawn their own.
- Commit messages, code, comments and docs use plain, neutral English.
- Every control gets a hover text. Actions report what they did, or why they were skipped.
- **Keep the user manual current**: every new feature or changed control updates
  `docs/manual` in the same change. Note user-visible changes under `## [Unreleased]` in
  `CHANGELOG.md`.

## Versions and releases

- One version for all crates: `[workspace.package] version` in Cargo.toml, `MAJOR.FEATURE.FIX`
  (major: incompatible files; feature: new features; fix: bug fixes and small improvements).
- `tools/release.py fix|feature|major [--push]` bumps, moves the changelog entries, commits
  and tags `vX.Y.Z`; the tag runs `.github/workflows/release.yml` (packages via
  `tools/package.py` + `packaging/`, release notes from the changelog section, PDF manual from
  `tools/build_manual.py`). Running the workflow by hand only builds the packages as
  artifacts. CI (`ci.yml`) runs rustfmt, clippy `-D warnings` and the tests without GPU.
- Development versions: stage N ends with tag `v0.(N+1).0`; `v0.*` tags only build the
  packages (workflow artifacts), no GitHub release. The first public release is 1.0.0, after
  all stages of PLAN.md.
- Linux builds run on Ubuntu 22.04 (glibc 2.35). macOS: arm64 and x86_64 `.app` bundles with
  an ad-hoc signature (unsigned); Windows: zip, no Authenticode.
- Platform folders: `rfr_core::paths`. OIDN is looked up in `RFR_OIDN_DIR`, next to the
  program, in `lib/`, `../lib` (AppImage), `../Frameworks` (macOS bundle) and `oidn/` of the
  source tree (`python3 tools/fetch_oidn.py`).
- Icon and manual cover: `python3 tools/make_icon.py` (Pillow).

## Layout

| Path | Content |
|------|---------|
| `crates/rfr-core` | `formula/` (`parse.rs` lexer and parsers for UFM syntax and FRM-like v1, `ast.rs` (program kinds), `check.rs` names/types → `ir.rs`, `cpu.rs` closures generic over the scalar with coloring hooks and transformations, `color.rs` formula colors, `library.rs` bundled sets, user formula folders indexed in the background and compiled on first use, `reload_file`), `formulas/` (`rfract/rfract.ufm` fractals, `rfract.ucl` colorings, `rfract.uxf` transformations, `fractint.ufm` FractInt escape-time types with FractInt's parameter layout, `fractalpark/` bundled set, `tools/import_fractalpark.py`), `math/` (`scalar.rs` f64 and dual numbers, `complex.rs` stdlib), `doc.rs` (.rfr model v2: layer tree, merge modes, masks), `gradient.rs` (OKLab gradients, opacity, LUT), `gradient_io.rs` (.ugr/.map/.ggr/.gpl/.cpt), `palette.rs` (generator, from RMandelbulb3D), `render/` (`mod.rs` plan tree, sample positions, coloring math, precision; `composite.rs` merge modes; `cpu.rs` reference renderer; `oidn.rs`; `post.rs` effects, `Glow` + `apply_region` for tiles; `tiles.rs` poster tiles), `io/` (.rfr, PNG with document, `upr.rs` and `par.rs` parameter file import into documents with notes), `animation.rs` (keyframes, view/camera path, timing incl. recorded flights, drivers, flight → keyframes, zoom sequence path), `expr.rs` (driver expressions), `audio.rs` (sound levels), `video.rs` (ffmpeg sink), `queue.rs` (render queue model), `explore.rs` (random documents, framing, mutations, thumbnail quality), `paths.rs` |
| `crates/rfr-gpu` | `codegen.rs` (layer plan → WGSL: fractal, transformations, colorings with local prefixes and parameter bases), `wgsl/` (`scalar_f32`, `scalar_dual`, `scalar_df64` layers, `cstd.wgsl` complex stdlib, `blend.wgsl` merge modes, `kernel.wgsl` layer kernel, `composite.wgsl` fill/merge/accumulate, `display.wgsl`), `renderer.rs` (op schedule with scratch slots, per-layer pipelines and buffers, progressive bands, readback), `animation.rs` (frame renderer with motion blur, stills, zoom sequence), `batch.rs` (queue jobs), `poster.rs` (tiled posters); `tests/shaders.rs` (all programs validate, GPU/CPU parity incl. layers) |
| `crates/rfr-app` | egui app: `app.rs` (layout, menus, files, shortcuts), `layers.rs` (layer tree, selection), `viewport.rs` (preview, navigation, Julia pick, color cycling), `inspector.rs` (layer/mask/fractal/transformation/coloring settings), `browser.rs` (formula list, user formulas by file, new formulas), `editor.rs` (formula editor), `palette_editor.rs` (from RMandelbulb3D; opacity, import/export), `export.rs`, `timeline.rs` (keyframes, flight recording, timing, drivers, sound), `anim_export.rs`, `queue_window.rs`, `explorer.rs` (explore lane), `doc.rs` (undo), `help.rs`/`manual.rs`, `theme.rs`, `widgets.rs`, `icons.rs`, `prefs.rs` |
| `crates/rfr-cli` | Command line (`render`, `animate`, `batch`, `formulas`, `formulas --check`, `params --check`, `info`) |
| `docs/manual` | User manual: `NN-*.md` chapters, `images/`, `manual.css` |
| `tools/` | `release.py`, `package.py`, `fetch_oidn.py`, `build_manual.py`, `make_icon.py`, `import_fractalpark.py` |
| `packaging/` | Linux desktop entry and AppRun, macOS Info.plist, INSTALL texts, release footer |
| `_dev/` | Reference only, gitignored: UFM formula database, FractInt source, `fractalpark` checkout (FRM-like v1 spec), `fract4d-formulas` (orgform, rights unclear: never bundled) |

## Build and test

```sh
cargo build -j 8 -p rfr-app -p rfr-cli
RAYON_NUM_THREADS=8 cargo test -j 8 --workspace -- --test-threads=8
cargo clippy -j 8 --workspace --all-targets -- -D warnings
cargo fmt --all --check
```

## Formula language and renderers

- One IR for all front ends; every node carries its type and conversions are explicit. The CPU
  back end (`formula/cpu.rs`) and the WGSL generator (`rfr-gpu/src/codegen.rs`) translate node
  by node; `math/complex.rs` and `wgsl/cstd.wgsl` implement the stdlib function by function in
  the same order. Change both sides together.
- The complex stdlib is written once over a scalar layer: Rust `Scalar` (f64, `Dual`), WGSL
  `s_*` functions (`scalar_f32`, `scalar_dual`, `scalar_df64`). Derivatives for distance
  estimation come from dual numbers, never from hand-written rules.
- Precision by zoom: `render::Plan::precision` (`SINGLE_LIMIT` 1e-6, `DOUBLE_LIMIT` 2e-15 of
  the coordinates per pixel, both measured); `rfr_gpu::renderer::renderer_mode` picks the shader.
- Smooth coloring: divergent orbits by the last radius with the growth rate estimated (and
  snapped to integers), convergent ones by the step before the last (f32 cannot resolve the
  last one). CPU and GPU must keep the same formulas (`render/mod.rs`, `kernel.wgsl`).
- The shader signature is the program, the chosen functions and the mode; parameter values,
  view and coloring are buffers (no recompiles while editing).
- `rfr-cli formulas --check _dev/<UFM folder>` reports how much of the UFM database compiles;
  `rfr-cli --formula-dir <folder> params --check <files> [--thumbs dir] [--notes]` converts
  `.upr`/`.par` files. Formulas of other programs' standard files map onto built-ins in
  `io/upr.rs` (`standard`), FractInt types in `io/par.rs` (`type_formula`).
- Classes (`.ulb`) are compiled away in `check.rs`: every `new` is an instance whose fields are
  locals `o<n>.<field>`, method calls are inlined (`inline`, `pending` statements before the
  using statement, `return` via result/done locals), plugin parameters are `<path>.<name>`.
  Object arrays are element lists (`<key>[k]` in `objects`): known indexes (`static_int`) pick
  the element, run-time indexes become `if index == k` chains (`dispatch`), counting `while`
  loops that make or index objects are unrolled (`try_unroll`; loops up to a bounded value,
  `bound_int`, unroll to the bound with a run-time guard per iteration; `setLength` with such a
  value keeps the length in a local, `obj_len`). An object variable is an
  `OVal`: one object or a tag (int local) with candidates; `if` arms, early returns and
  `dispatch` arms join with `merge` (tags set at the end of each branch), `store_if` stores
  under a run-time condition, uses become `(condition, object)` alternatives (`obj_alts`).
  `library::class_registry()` holds the classes, `Entry::program_with(params)` compiles per
  plugin choice. Dev tools: `cargo run -p rfr-core --example check_entry -- <file> <entry>
  [<param>=<class> …]` (compile one entry with its folder's classes and chosen plugins, trace
  orbits), `--example ulb_errors <dir>`.
- Image parameters (`ParamKind::Image`, value: file path): `new @image` is a built-in image
  object (`Inst::image`); its methods become `ColorOp::Image*` ops. Images load through
  `rfr_core::images` (cache, sampling); the GPU gets them per layer as a packed `u32` buffer
  (binding 4, slots in `codegen::layer_images` order, `k_image_*` in `kernel.wgsl`).
- Arrays live per sample (`ir::Local::cap`, at most 4096 elements); colors are `Ty::Color`
  (sRGB-encoded RGBA, `ColorOp`), never converted to numbers (`check::color_misuse`).
- Colorings and transformations are programs of the same IR (`ProgramKind`): colorings run
  `init` after the fractal's init, `loop` after every iteration that continues, `final` at the
  end, and write `#index`/`#solid`; transformations change `#pixel` (local slot 0) before the
  fractal. The CPU (`render/cpu.rs` `LayerK::eval`) and the GPU kernel (`codegen::kernel`) run
  them in the same order.
- Layers: `Plan` is a tree (`Node::Layer`/`Group`, masks); `Plan::layers()` is the render order
  (mask before its layer) that `rfr_gpu::renderer::schedule` and `CpuRenderer` share.
  Compositing (`render/composite.rs`, `wgsl/blend.wgsl`) works on sRGB-encoded colors with
  straight alpha; the image accumulates premultiplied linear RGBA.

## Design rules (from PLAN.md)

- CPU and GPU implement the same algorithm step by step; change both together; parity tests.
- One formula source (language text in `formula/lib`), never written twice per back end.
- Parameters and settings live in buffers: editing never recompiles shaders.
- Never name commercial fractal programs anywhere in the repository, the program or its
  documentation (code comments and commit messages included). Name file formats by their
  extensions (UFM: `.ufm`, `.ucl`, `.uxf`, `.ulb`, `.upr`). Inspiration notes live only in the
  gitignored `_dev/inspiration.md`.
- FractInt code is read for algorithms only, never copied. Bundle only formula sets with a clear
  license (PLAN.md section 8); credit them in NOTICE, THIRD_PARTY_NOTICES.md and Help › About.

## User manual

- `docs/manual/NN-*.md` (one chapter per file, sorted by name; `# ` chapter, `##`/`###`
  sections; `![Caption](images/x.png)` alone in a paragraph is a figure, `{: .half}` makes it
  half wide; `!!! tip` / `!!! note` / `!!! warning` boxes; `{{version}}`). Conventions as in
  the RMandelbulb3D manual.
- The app embeds every chapter and image (`rfr-app/build.rs` → `manual.rs`, Help › Manual).
  A test checks that every referenced image exists.
- PDF: `python3 tools/build_manual.py` (Python `markdown`, Chrome, pdftotext) →
  `docs/manual/build/RFract-Manual-<version>.pdf` (gitignored).

## Testing the app visually

- Xvfb on `:99` (1600×1000), separate `XDG_CONFIG_HOME`; `xdotool` + `import -window root`.
  Without a window manager keys go nowhere until the window has focus: click into it first.
  Stop with `pkill -x rfr-app`.
