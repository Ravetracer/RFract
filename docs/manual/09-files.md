# Files, export and the command line

## Documents

**Ctrl+S** saves the document as `.rfr` (a readable text file with all settings),
**Ctrl+O** opens `.rfr` files and PNG images rendered by RFract. Drop a file onto the window to
open it. RFract asks before it discards unsaved changes.

Image parameters store the image file's path. When the file is not there (a document or
parameter file from another computer), RFract looks for a file of the same name in your
formula folders.

## Parameter files (.upr, .par)

**Ctrl+O** also opens `.upr` and FractInt `.par` parameter files and FractInt `.ifs` files
(iterated function systems) and `.l` files (L-systems), see *Density layers*. A file with several images shows their list
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

FractInt `.par` files bring the fractal type with its parameters and functions, the view
(`corners=`, including rotated corners, or `center-mag=`), the iteration limit, the inside
color (a palette entry, `maxiter`, `bof60`, `bof61`, `zmag`, `atan`, `epscross`) and the
256-color palette, colored by the iteration count as in FractInt. A palette given as
`colors=@name.map` is looked up next to the parameter file, in a `maps` folder beside it and
in the formula folders; without a palette the image uses FractInt's default (VGA) palette.
`type=formula` images take their formula from the formula folders (`formulafile=`,
`formulaname=`). The escape-time types are RFract's *FractInt types* (see the Formulas
chapter); orbit, attractor, IFS, L-system and cellular types are not available yet.

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

`rfr-cli animate` renders a document's animation into a movie or PNG images (see
*Animation*).

`render` also takes a `.upr` file; `--entry <name>` picks its image (default: the first),
and the notes of the conversion go to the terminal.

Documents that use formulas from your formula folders need the folders on the command line
too: `--formula-dir <folder>` (once per folder) makes their formulas available, with the same
ids as in the app:

```
rfr-cli --formula-dir ~/formulas render picture.rfr -o picture.png
rfr-cli --formula-dir ~/formulas render "user:fractint.frm:Daisy" -o daisy.png
```
