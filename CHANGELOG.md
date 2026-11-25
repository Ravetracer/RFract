# Changelog

User-visible changes per version. Versions are MAJOR.FEATURE.FIX (see AGENT.md).

## [Unreleased]

## [0.2.0] - 2026-11-25

Stage 1: escape-time core and viewer.

### Added

- Formula language with two front ends: UFM syntax (RFract's own formulas) and FractalPark
  FRM-like v1; one typed IR, compiled for the CPU and generated as WGSL for the GPU.
- 552 built-in formulas: 18 RFract formulas (Mandelbrot, Multibrot, Burning Ship, Tricorn,
  Celtic, Buffalo, Perpendicular, Phoenix, Lambda, Magnet I/II, Newton, Nova, Spider, Manowar,
  Barnsley 1–3) and the 534 FractalPark formulas.
- GPU renderer with progressive anti-aliasing, automatic double-float precision for deep views
  (clean to about 10¹² at full HD) and automatic derivatives for distance estimation; CPU
  reference renderer with the same results.
- Coloring: smooth iteration count (divergent and convergent formulas), bands, distance
  estimation, solid; inside solid, magnitude or angle; transfer, density and offset.
- Gradients blended in OKLab with presets; gradient editor with generator, adjustments and
  tools (from RMandelbulb3D).
- View navigation: drag, wheel zoom at the cursor, box zoom, rotation, double-click zoom,
  Julia sets by Alt+click or J, reset view.
- Formula browser with search, inspector for parameters, iterations, Julia mode, coloring,
  view and image size; undo/redo for every change including the view.
- Documents (.rfr), PNG export with the embedded document (opens again as its settings).
- rfr-cli render (GPU, or --cpu), formulas, formulas --check.

## [0.1.0] - 2026-11-25

Stage 0: foundation and distribution.

### Added

- Program shell: window with the final panel layout (layers, inspector, timeline, explore
  lane, viewport), preferences (UI scale, font size), help window with the embedded manual.
- `rfr-cli info`: version, graphics card, denoiser and settings folders; `rfr-cli --version`.
- Release packages for Linux (tar.gz, AppImage), Windows (zip) and macOS (arm64 and x86_64,
  unsigned) built by GitHub Actions, with the denoiser and the PDF manual.
- FractalPark's published formula library (534 formulas, MIT) bundled as source for the
  built-in formulas; credited in NOTICE, THIRD_PARTY_NOTICES.md and Help › About.
