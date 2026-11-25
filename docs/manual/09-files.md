# Files, export and the command line

## Documents

**Ctrl+S** saves the document as `.rfr` (a readable text file with all settings),
**Ctrl+O** opens `.rfr` files and PNG images rendered by RFract. Drop a file onto the window to
open it. RFract asks before it discards unsaved changes.

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

Documents that use formulas from your formula folders need the folders on the command line
too: `--formula-dir <folder>` (once per folder) makes their formulas available, with the same
ids as in the app:

```
rfr-cli --formula-dir ~/formulas render picture.rfr -o picture.png
rfr-cli --formula-dir ~/formulas render "user:fractint.frm:Daisy" -o daisy.png
```
