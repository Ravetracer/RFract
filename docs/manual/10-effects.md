# Effects

The *Effects* section (below *Image* in the inspector) changes the finished picture, the way a
camera and its film would: brightness and tone, colors, glow and lens effects. The effects act
on the whole image after the layers are combined. Editing them does not render anew, so the
sliders answer at once; the preview and the exported image show the same.

## Tone

- **Exposure** brightens or darkens in stops: +1 doubles the light.
- **Tone mapping** brings light brighter than white back into range. *None* cuts it off;
  *Reinhard* is soft and keeps colors; *ACES* gives a film-like curve with rich contrast;
  *Filmic* (Hable's curve) has a gentle shoulder and lifted shadows; *AgX* lets very bright
  colors fade toward white like film does.
- **Contrast** and **Saturation**: 1 leaves the image unchanged.

## Color

- **Temperature** and **Tint** set the white balance: cooler or warmer, greener or more
  magenta. Gray keeps its brightness.
- **Split toning**: the dark parts take on the *Shadow tone*, the bright parts the *Highlight
  tone*, each by its amount.

## Glow

- **Bloom** spreads a soft glow around parts brighter than the *Bloom threshold* (after the
  exposure; 1 is white). *Bloom size* goes from a tight glow to a wide haze. Bright
  colorings, lit highlights and density layers glow best.
- **Streaks** draw lines of light through very bright parts, like a star filter: *Streak
  directions* 1 gives a line, 2 a cross, 3 and 4 stars; length, angle and tint as you like.

Glow also spreads over transparent parts of the image, which then become partly opaque.

## Denoise

**Denoise exports** runs exported images and command line renders through Intel Open Image
Denoise before the other effects: grain from few samples (soft shadows, occlusion, glows,
density layers) disappears. The preview is not denoised. The denoiser also softens very fine
detail such as thin filaments; *Denoise amount* below 1 keeps part of the original. The export
message says whether the image was denoised and on which device; without the library (see
*Installation*) the image is saved as it is.

## Lens and film

- **Sharpen** makes edges crisper.
- **Distortion** bends the image like a wide-angle lens (barrel, to the right) or a telephoto
  lens (pincushion, to the left).
- **Chromatic aberration** lets red and blue drift apart toward the edges.
- **Vignette** darkens the corners, **Grain** adds a fixed film grain.

The reset button of the section switches every effect off.
