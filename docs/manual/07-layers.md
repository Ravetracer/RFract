# Layers

An image is a stack of layers. Every layer computes its own fractal (formula,
transformations, colorings) over the same view, and the stack combines them from the bottom
up, over the background color.

![Layers: a triangle inequality base and Pickover stalks in Screen mode](images/example-layers.jpg)

## The layer list

![The layer list above the formula browser](images/layers-window.png)

The list shows the top of the stack first. Click a layer to edit it: the formula browser and
the inspector work on the selected layer.

| Button | Action |
|--------|--------|
| + | A new fractal layer above the selected one (same colors, Screen mode) |
| Folder | A new group above the selected layer |
| ↑ ↓ | Move the selected layer up or down |
| Bin | Delete the selected layer (or its mask) |
| … | Duplicate, group, ungroup, add a mask |
| Eye | Show or hide a layer |

## Merge modes and opacity

The *Layer* section of the inspector sets how a layer combines with the layers below it:

- **Opacity** from 0 (hidden) to 1.
- **Merge mode**: Normal, Multiply, Screen, Overlay, Soft light, Hard light, Color dodge,
  Color burn, Darken, Lighten, Difference, Exclusion, Add, Subtract, Hue, Saturation, Color
  and Luminosity. They work like the layer modes of image editors (on sRGB colors, after the
  W3C compositing rules); the hover text of each mode describes it.

Transparent parts of a layer let the layers below show through: gradients can have an opacity
curve (see Coloring), solid colors an opacity, and transformations can leave points empty.

## Groups

A group combines its layers first and then merges the result like a single layer, with its
own merge mode, opacity and mask. Groups can hold groups.

## Masks

A mask is a fractal of its own that decides where its layer (or group) shows: bright parts of
the mask show the layer, dark parts hide it (or the other way round with **Invert**). With
**Alpha** as the source, the mask's opacity decides instead of its brightness.

Add a mask with *Add a mask* in the inspector or the … menu. The mask appears under its layer
in the list; select it to give it a formula from the browser and colors like any layer. Its
merge mode and opacity do not matter.

## Background

*Image › Background* is the color below all layers. With its opacity at 0, exported PNGs keep
the transparency, and the view shows a checkerboard where the image is transparent.
