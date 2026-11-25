# Files, export and the command line

## Documents

**Ctrl+S** saves the document as `.rfr` (a readable text file with all settings),
**Ctrl+O** opens `.rfr` files and PNG images rendered by RFract. Drop a file onto the window to
open it. RFract asks before it discards unsaved changes.

## Parameter files (.upr)

**Ctrl+O** also opens `.upr` parameter files. A file with several images shows their list
(with a search); click one to open it as a new document. RFract converts the layers (caption,
opacity, merge mode, visibility), the view (center, magnification, rotation), the formulas
with their parameters, the inside and outside colorings, the transformations and the gradient
with its opacity.

The formulas come from your formula folders (see *Formula files* in the Formulas chapter),
found by file name and entry. Images made with the standard formulas that came with another
program use RFract's built-in equivalents where there is one (Mandelbrot, Julia, the iteration
count, smooth and distance colorings, orbit traps, exponential smoothing, decomposition,
Gaussian integers, the triangle inequality average, inversion, kaleidoscope, mirror and
twist). Whatever does not carry over exactly is listed in the *Import notes* window (a ⚠ in
the image list marks these images): a layer whose formula is missing shows the Mandelbrot set
instead.

!!! note
    Differences to expect: one view for all layers (the bottom layer's), no stretched or
    skewed views, and colors that may differ in density where a standard coloring is mapped
    onto a built-in method. Parameter files stored in the compressed format are not read.

## Export

**Ctrl+E** renders the image at the size and samples per pixel set in the *Image* section
and saves a PNG. The PNG carries the document, so every exported picture opens again with all
its settings. More samples smooth the edges (anti-aliasing); 64 is a good start, fine detail
may need 256 or more.

## Command line

`rfr-cli` renders without the window:

```
rfr-cli render picture.rfr -o picture.png
rfr-cli render picture.png -W 3840 -H 2160 -s 256 -o big.png
rfr-cli render rfract:BurningShip -o ship.png
rfr-cli formulas
rfr-cli info
```

`render` takes a document, a PNG rendered by RFract or a formula id (`rfr-cli formulas` lists
them) and renders on the graphics card; `--cpu` uses the CPU reference renderer instead.
`info` shows the graphics card, the denoiser and the settings folder.

`render` also takes a `.upr` file; `--entry <name>` picks its image (default: the first),
and the notes of the conversion go to the terminal.

Documents that use formulas from your formula folders need the folders on the command line
too: `--formula-dir <folder>` (once per folder) makes their formulas available, with the same
ids as in the app:

```
rfr-cli --formula-dir ~/formulas render picture.rfr -o picture.png
rfr-cli --formula-dir ~/formulas render "user:fractint.frm:Daisy" -o daisy.png
```
