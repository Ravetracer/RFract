# Introduction

RFract is a 2D fractal generator. It builds images from layers of fractals – each with its own
formula, coloring, gradient, merge mode and mask – renders them on the graphics card, and turns
them into stills and animations with glow, chromatic aberration, lighting and denoising.

![RFract](images/cover.jpg)

This manual describes version {{version}}, a development version: layers with merge modes,
groups and masks, more than 600 built-in formulas and the formula language with its files,
colorings and transformations, gradients with opacity, density layers (Buddhabrot, attractors,
IFS, L-systems), lighting with 3D terrains and 3D objects, deep zooms to any depth, effects
with denoising, animations with keyframes, flights, drivers and sound, posters of any size, a
render queue and an explore lane for new ideas.

*How-tos* at the end walks through complete projects: a deep zoom movie, a poster, a
landscape fly-over, a music video and more.

## Related programs

RFract is the third program of a family:

- **RWildFire** – fractal flames (a Rust port of JWildfire).
- **RMandelbulb3D** – 3D fractals (a Rust re-implementation of Mandelbulb3D).
- **RFract** – 2D fractals with layers.

The three share their look, their way of working and the denoiser.
