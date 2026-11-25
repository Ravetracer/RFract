# Introduction

RFract is a 2D fractal generator. It builds images from layers of fractals – each with its own
formula, coloring, gradient, merge mode and mask – renders them on the graphics card, and turns
them into stills and animations with glow, chromatic aberration, lighting and denoising.

![RFract](images/cover.jpg)

This manual describes version {{version}}. RFract is at an early stage: this version is the
program shell (window, panels, preferences, help) and the release packages for Linux, Windows
and macOS. Rendering, layers, formulas and animation follow in the next versions.

## Related programs

RFract is the third program of a family:

- **RWildFire** – fractal flames (a Rust port of JWildfire).
- **RMandelbulb3D** – 3D fractals (a Rust re-implementation of Mandelbulb3D).
- **RFract** – 2D fractals with layers, in the spirit of Ultra Fractal and FractInt.

The three share their look, their way of working and the denoiser.
