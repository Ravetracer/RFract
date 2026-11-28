# How-tos

Complete projects from start to finish. Each step links to the chapter with the details.

## A deep zoom movie

1. Open *View › Timeline*. Start at the whole set (**Home**) with few iterations (500) and
   press **Add**: keyframe 1.
2. Zoom into a place you like (mouse wheel, box zoom with Shift+drag). Deep views switch to
   double precision and then to perturbation by themselves (*The view › Precision*). Raise
   the iterations while you go deeper: the edge needs more of them. Press **Add**: keyframe 2.
   Iterations blend on a logarithmic scale between the keyframes.
3. Choose **Steady** timing, a **Length** (60 s) and a second or two of **Ease in** and
   **Ease out**. The zoom moves at an even pace and the target stays in place.
4. Optional: **Palette rotation** (0.05 turns per second) and **Motion blur** (0.5).
5. Set the image size and samples (*Image*: 1920 × 1080, 16–64 samples; *Adaptive* keeps the
   time down) and press **Export animation…** for an MP4.

For very long zooms without palette rotation, **Zoom sequence** renders one image per doubling
of the zoom instead of every frame: many times faster. On the command line the same document
renders with `rfr-cli animate zoom.rfr -o zoom.mp4`.

!!! tip
    Points that stay interesting at every depth are on the edge of the set; Misiurewicz points
    (where branches meet) look the same at every depth. The *Deep zoom target* preset of the
    explore lane is one, good to 1e-40.

## A poster

1. Frame the image in the view and choose the colors.
2. Set the size in *Image* (for example 20000 × 11250) and the samples (64–256).
3. Press **Ctrl+E**. Above 8K UHD the image renders in tiles and goes to the PNG a band at a
   time; the export window shows the tiles. A 20000 × 11250 poster with 4 samples takes about
   a minute on a laptop graphics card.

Posters in tiles work with every kind of layer; density layers take a second sweep over the
tiles. See
*Files › Posters*.

## A landscape fly-over

1. Pick the Mandelbrot set (or any escape-time fractal) and open *Lighting* in the inspector:
   switch on **Lit**, choose **Distance** for the heights and switch on **3D terrain**. The
   *Landscape* preset of the explore lane is a ready start.
2. Ctrl+drag orbits the camera, Ctrl+wheel moves it closer. Zoom and move the view to choose
   the part of the fractal under the camera.
3. In the timeline press **Record flight** and fly: move the view, orbit the camera. Press
   **Stop recording**: keyframes follow the flight with its timing. Raise **Cameraman** for a
   smoother flight and press **Keyframes again**.
4. Export as in the deep zoom.

## A music video

1. Make an animation (keyframes or a recorded flight) as long as the music.
2. In the timeline press **Sound…** and choose the music; RFract analyses it once.
3. Add drivers that follow the music: for example `post.exposure` with `value + 0.8 * bass`,
   `layers.0.outside.offset` with `value + 0.2 * audio`, or a formula parameter with
   `value + 0.1 * treble`. Each driver shows its curve.
4. Export: the sound goes under the movie.

## Finding new images

1. Open *View › Explore lane* and choose **Random**, a kind (escape time, Julia sets, root
   finding, lit reliefs) and **New batch**.
2. Click a thumbnail you like: it becomes the document. Keep it with the camera button
   (*Snapshots*).
3. Switch to **Mutations**, choose what may change and press **Mutate**; take the best
   variant and mutate again. **Strength** sets how far the variants go.
4. Save the documents you want to keep (**Ctrl+S**).

## Images from other programs

Parameter files of other programs (`.upr`, `.par`) open with **Ctrl+O**: pick the image from
the file and RFract converts it, with notes on what does not carry over. Their formulas come
from your formula folders: add the folders under *File › Preferences › Formula folders* first (see
*Formulas › Your formula folders*).

## Rendering overnight

1. Set up the documents and save them.
2. *File › Render queue…* → **Add document files…** (or **Add this image…** / **Add this
   animation…** for the open document). The gear button of a job sets its size, samples,
   frames and output.
3. Press **Start**. The queue stays when RFract closes; `rfr-cli batch` renders the same queue
   file (or a list of documents) without the window.
