# Animation

An animation moves from one **keyframe** to the next. A keyframe stores the whole document:
the view, the layers with their formulas, colors, lights and effects. Between keyframes every
number blends smoothly; texts and choices (a formula, a coloring method) change at the
keyframe. The *Timeline* (View › Timeline) shows the animation below the view.

## Keyframes

Set up the view and the layers as the animation should start and press **Add** in the
timeline: the document becomes keyframe 1. Zoom, move, change colors or parameters and
**Add** again for keyframe 2, and so on. A new keyframe goes after the selected one.

Click a keyframe (its number in the list or its diamond on the time bar) to load it into the
document; change it and press **Update** to store the change, **Delete** to remove it.

The view follows its own path: the zoom moves at an even pace (each doubling takes the same
time), and while zooming the next keyframe's center stays at the same place on the screen,
so a zoom heads straight for its target; this stays exact at any zoom depth. Rotations turn
the short way round, as do angles of lights and cameras. Distances (camera distances,
iteration counts) blend on a logarithmic scale.

## Recording a flight

Instead of setting keyframes by hand you can fly: press **Record flight** in the timeline,
navigate (fly mode, dragging, zooming, orbiting a terrain camera: everything that changes the
view or a 3D camera counts) and press **Stop recording**. The still time before the first and
after the last move is cut off. RFract places as few keyframes as needed along the flight and
switches the timing to *As flown*.

The *FLIGHT* column sets how the recording becomes keyframes:

- **Cameraman** smooths the flight like a camera operator: zooming, moving, turning and the
  3D cameras are evened out over this many seconds, so jerks of the mouse become smooth moves
  (0: exactly as flown). The start and the end stay where they were.
- **Tolerance**: how far the path through the keyframes may stray from the flight (a share of
  the view): smaller values give more keyframes that follow the flight more closely.
- **Keyframes again** makes the keyframes anew with these settings; **Forget** drops the
  recording (the keyframes stay). The recording lives until RFract closes; the keyframes are
  saved with the document. Settings other than the view and the cameras come from the
  document as it is when the keyframes are made.

## Playing and scrubbing

Click or drag on the time bar to look at a frame; ▶ plays the animation in the view at fast
preview quality. While a frame is shown the document is not edited: **Edit** goes back to it.
The time bar marks the seconds and the keyframes.

## Timing

- **Per keyframe**: each keyframe sets the frames to the next one and how they are spread
  (*Even*, *Slow in and out*, *Slow start*, *Slow end*).
- **Steady**: one steady speed over the whole *Length*, as it looks on the screen (zooming,
  moving relative to the view and turning together), with *Ease in* and *Ease out* seconds
  to speed up at the start and slow down at the end.
- **As flown**: the pace of a recorded flight (each keyframe keeps its time), with speed
  changes evened out over *Smooth speed* seconds (0: exactly as flown).
- **Frames per second** of the movie.
- **Palette rotation** turns all gradients over time (turns per second).
- **Motion blur** exposes each frame for a share of the frame time (0.5: a film camera's
  180° shutter): the samples of a frame spread over that time, so fast zooms and turns blur
  as in a film.

## Zoom sequence

Long zooms into the depths take many frames, and every frame renders the fractal anew.
**Zoom sequence** makes them much faster: the export zooms straight from the first
keyframe's size into the last keyframe (its center and all its settings), at the steady
pace of *Length* with *Ease in* and *Ease out*. It renders one image per doubling of the zoom,
at twice the frame size, and composes every frame from the two images around its zoom:
the finer one in the middle, the coarser one around it. A zoom by 10³⁰ needs about 100
images however many frames it has. Motion blur still works (the images are blended at
several zooms within the shutter); palette rotation does not (the colors are in the
images), and the keyframes between the first and the last are not used.

## Drivers

A driver computes a value of the document in every frame with an expression, after the
keyframes. Press **+ Driver**, choose the value with **…** (a search over all numbers of the
document, formula parameters included, e.g. `layers.0.params.power` or `view.rotation`) and
write the expression. Below each driver a small curve shows its value over the animation;
errors show there in yellow.

- Variables: `t` (seconds), `frame`, `frames`, `u` (0 at the start, 1 at the end), `key`
  (keyframe position), `fps`, `value` (the value from the keyframes), `audio`, `bass`, `mid`,
  `treble` (the sound's levels 0..1, see *Sound*).
- Functions: `sin cos tan asin acos atan atan2 sqrt abs exp ln log10 pow min max clamp mix
  step smoothstep floor ceil round fract sign noise if deg rad`; comparisons give 1 or 0.
- `@path` reads another value of the document: `@view.rotation / 1000`.

Examples: `value + 0.5 * sin(t * 2)` lets a value breathe, `value + 0.1 * noise(t)` wobbles
it slowly, `value + bass` pumps the exposure with the music. Integers (iterations) are
rounded, switches are on above 0.5.

## Sound

**Sound…** puts a sound file under exported movies; **Sound offset** sets where in the file
frame 0 starts (negative: the sound starts later). RFract analyses the sound once with
ffmpeg (loudness, bass, mids and treble, smoothed) and stores the levels in the document, so
drivers can follow the music in the preview, the export, the render queue and on the command
line. A relative sound path is relative to the document file.

## Export

**Export animation…** renders every frame at the document's image size and samples (see
*Image*) and writes a movie: `.mp4` (H.264), `.mov` (ProRes), `.webm` (VP9) or `.gif`; a
name without extension writes numbered PNG images into a folder of that name. Movies need
[ffmpeg](https://ffmpeg.org) installed (on the search path or next to RFract). The effects
(see *Effects*) apply to every frame and can change from keyframe to keyframe; *Denoise
exports* denoises every frame.

On the command line:

```
rfr-cli animate zoom.rfr -o zoom.mp4
rfr-cli animate zoom.rfr -o frames -W 1920 -H 1080 -s 64 --format png
rfr-cli animate zoom.rfr -o zoom.mp4 --format av1 --from 100 --to 199
```

`--format` also takes `hevc` (H.265 in MP4), `av1`, `prores`, `webm` and `gif`; `--from`
and `--to` render part of the frames; `--sequence` renders a zoom sequence even when the
animation does not ask for one. A zoom sequence uses the last keyframe's settings: drivers
do not apply to it.

To render several animations one after the other, use the render queue (see *Files*).
