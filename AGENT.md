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
| `crates/rfr-core` | `formula/` (`parse.rs` lexer and parsers for UFM syntax and FRM-like v1, `ast.rs`, `check.rs` names/types → `ir.rs`, `cpu.rs` closures generic over the scalar, `library.rs` bundled sets), `formulas/` (`rfract/rfract.ufm` built-ins, `fractalpark/` bundled set, `tools/import_fractalpark.py`), `math/` (`scalar.rs` f64 and dual numbers, `complex.rs` stdlib), `doc.rs` (.rfr model), `gradient.rs` (OKLab gradients, LUT), `palette.rs` (generator, from RMandelbulb3D), `render/` (`mod.rs` plan, sample positions, coloring math, precision; `cpu.rs` reference renderer; `oidn.rs`), `io/` (.rfr, PNG with document), `paths.rs` |
| `crates/rfr-gpu` | `codegen.rs` (IR → WGSL), `wgsl/` (`scalar_f32`, `scalar_dual`, `scalar_df64` layers, `cstd.wgsl` complex stdlib, `kernel.wgsl`, `display.wgsl`), `renderer.rs` (progressive bands, async compile, readback); `tests/shaders.rs` (all formulas validate, GPU/CPU parity) |
| `crates/rfr-app` | egui app: `app.rs` (layout, menus, files, shortcuts), `viewport.rs` (preview, navigation, Julia pick), `inspector.rs`, `browser.rs` (formula list), `palette_editor.rs` (from RMandelbulb3D), `export.rs`, `doc.rs` (undo), `help.rs`/`manual.rs`, `theme.rs`, `widgets.rs`, `icons.rs`, `prefs.rs` |
| `crates/rfr-cli` | Command line (`render`, `formulas`, `formulas --check`, `info`) |
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
- `rfr-cli formulas --check _dev/<UFM folder>` reports how much of the UFM database compiles.

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
