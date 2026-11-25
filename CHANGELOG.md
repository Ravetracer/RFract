# Changelog

User-visible changes per version. Versions are MAJOR.FEATURE.FIX (see AGENT.md).

## [Unreleased]

### Added

- Program shell: window with the final panel layout (layers, inspector, timeline, explore
  lane, viewport), preferences (UI scale, font size), help window with the embedded manual.
- `rfr-cli info`: version, graphics card, denoiser and settings folders; `rfr-cli --version`.
- Release packages for Linux (tar.gz, AppImage), Windows (zip) and macOS (arm64 and x86_64,
  unsigned) built by GitHub Actions, with the denoiser and the PDF manual.
- FractalPark's published formula library (534 formulas, MIT) bundled as source for the
  built-in formulas; credited in NOTICE, THIRD_PARTY_NOTICES.md and Help › About.
