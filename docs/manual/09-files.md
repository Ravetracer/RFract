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

**Adaptive** sampling (*Image* section) saves most of that time: after 32 samples a pixel
stops once it and its four neighbours are smooth, that is when the noise of its brightness is
below the *Adaptive* value. Flat parts finish early, edges and fine detail get every sample.
At the default of 0.005 a 1080p image with 256 samples renders about three times faster and
looks the same; 0 gives every pixel every sample. It works in the preview, exports, animations
and the command line (not for density layers, whose image restarts while it fills).

### Posters

Images larger than about 8K UHD (7680 × 4320 pixels) render in tiles of 2048 × 2048 pixels,
one after the other, so a poster of 20000 pixels or more fits on any graphics card and in
memory: the PNG is written a band of tiles at a time. Every tile is a part of the whole
image, with the same sample positions and random streams, so the tiles join without seams; the
effects work across them (lens distortion and chromatic aberration see past the tile edges,
the denoiser works tile by tile with an overlap). Bloom, streaks, god rays and lens flares
come from a smaller render of the whole view (3840 pixels on the long side), so they look as
in an image of that size.

Every kind of layer tiles. A lit layer's heights are computed around each tile as far as its
shadows and effects reach. A terrain's ground is the whole view, at most 4096 cells on its long
side, so a poster shows the ground of a 4K render in finer pixels. A density layer runs all its
orbits for every tile and keeps the points that land there; a first sweep over the tiles finds
the largest counts of the whole image for the tone curve, so a poster with density layers takes
about twice as long per tile as one without (raise *Orbits per pass* for large posters: the same
orbits spread over more pixels). The command line (`rfr-cli render -W 20000 -H 11250 …`)
and the render queue make posters the same way.

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
The CPU renderer (also used without a graphics card) guesses: blocks whose border lies inside
the set with one color are filled without computing them, which makes views with large
inside areas two to three times faster. Formulas whose inside has holes can lose small details
that way; `--no-guess` (before the command, for all of them) computes every pixel.
`info` shows the graphics card, the denoiser and the settings folder.

`rfr-cli animate` renders a document's animation into a movie or PNG images (see
*Animation*). `rfr-cli batch` works through a render queue file or a list of documents (see
*Render queue*).

`--bookmark <name>` renders the view of one of the document's bookmarks (see *View*).

`rfr-cli bench` renders a fixed set of documents (deep zooms, terrains, 3D objects, density
layers, coloring formulas) and prints the median time of each, for comparing versions or
settings on one computer; `rfr-cli bench picture.rfr` times your own documents at their
size and samples.

`render` also takes a `.upr` file; `--entry <name>` picks its image (default: the first),
and the notes of the conversion go to the terminal.

Documents that use formulas from your formula folders need the folders on the command line
too: `--formula-dir <folder>` (once per folder) makes their formulas available, with the same
ids as in the app:

```
rfr-cli --formula-dir ~/formulas render picture.rfr -o picture.png
rfr-cli --formula-dir ~/formulas render "user:fractint.frm:Daisy" -o daisy.png
```

## Render queue

![The render queue with the settings of a poster job](images/render-queue.png)

*File › Render queue…* renders images and animations one after the other, for example
overnight. **Add this image…** and **Add this animation…** queue a copy of the current
document as it is now (you can keep editing it), with the file to write; **Add document
files…** queues documents from disk, read when their turn comes: their animation as an MP4
when they have keyframes, else a PNG image, saved next to them. Images and animations use the
document's image size and samples.

The gear button of a job opens its settings: name, size (0 takes the document's; one side 0
keeps the aspect), samples per pixel (0: the document's), the frames of an animation and the
output file. Changing a finished job sets it to waiting again. Images above 8K UHD render in
tiles (see *Posters*).

**Start** renders the waiting jobs from the top while the preview pauses; **Stop** pauses the
current job. Move jobs up and down, **Pause** one to skip it, **Again** to render it once
more. The queue is kept in the settings folder and stays when RFract closes; a job that was
running then starts again.

On the command line `rfr-cli batch` renders the waiting jobs of that queue file
(`queue.json` in the settings folder, see `rfr-cli info`) or documents:

```
rfr-cli batch ~/.config/rfract/queue.json
rfr-cli batch zoom.rfr flight.rfr picture.rfr --out renders -W 1920 -H 1080
```

## Network rendering

Other computers of the local network can help with long renders. On each of them start a
render client (the same RFract version):

```
rfr-cli client
rfr-cli client --name studio --key secret
```

The client announces itself on the network and waits; it renders on its graphics card
(`--cpu`: on the processor) for whichever server connects, then waits for the next job.
Stop it with Ctrl+C. `--key` lets only servers that know the key use it.

*File › Network render…* lists the clients it hears (new ones start chosen; busy ones and
other versions are shown but cannot be chosen). Clients the announcements do not reach
(another subnet, a network that blocks broadcasts) can be added by address (`host` or
`host:port`). **This computer too** renders on this computer as well (the preview pauses
meanwhile). Choose the animation or the image, the size, the samples, the frames and the
output, then **Render on … clients**; the window shows the progress of every client.

- **Animations** go out frame by frame: every client renders whole frames (with motion blur,
  palette rotation and the effects), the server collects them and encodes the movie after
  the last one. In the zoom sequence mode a client renders a run of frames in a row, since
  its images carry over from frame to frame.
- **Images** go out in tiles of 512 pixels: every client renders tiles (exactly the pixels of
  the whole image there), the server denoises them, adds the effects and the glow and writes
  the PNG a band of rows at a time. With density layers the clients first count the
  densities of every tile, then render the tiles with the whole image's largest counts.

Fast clients take more pieces than slow ones; at the end an idle client also takes over the
piece that would take longest elsewhere, and the first result counts. A client that drops out
gives its piece back to the others; a finished piece it could not deliver is sent when it
comes back. The server sends every file the document needs: images of image parameters and
environments, and the formula files from your formula folders that the document uses (with
the class libraries when they need them). Clients keep received files by content in their
cache folder, so a second job sends only what changed.

On the command line:

```
rfr-cli net-clients
rfr-cli net-render zoom.rfr -o zoom.mp4 --local
rfr-cli net-render poster.rfr --still -W 7680 -H 4320 --clients studio,attic
rfr-cli net-render zoom.rfr --client 192.168.1.20 --key secret
```

`net-render` renders the animation when the document has one (`--still`: the image), with
the options of `animate`; `--clients` picks clients by name (default: all found within
`--wait` seconds), `--client` adds one by address, `--local` renders on this computer too.

!!! note
    The clients use UDP port 47820 for the announcements and TCP port 47821 for the
    servers; a firewall must let them through on the clients' computers.
