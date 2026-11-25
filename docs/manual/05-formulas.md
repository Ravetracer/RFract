# Formulas

A formula describes one iteration step: how *z* changes from one step to the next, where it
starts and when it has escaped (the bailout condition). RFract ships 552 formulas:

- **RFract** (18): Mandelbrot (integer powers), Multibrot (real powers), Burning Ship, Tricorn,
  Celtic, Buffalo, Perpendicular Mandelbrot, Phoenix, Lambda, Magnet I and II, Newton, Nova,
  Spider, Manowar and three Barnsley formulas.
- **FractalPark** (534): the published formula library of the open source FractalPark project
  (MIT license), grouped into families such as algebraic powers, transcendental functions,
  function compositions, rational functions, orbit memory, folded absolute values and root
  finding.

![Burning Ship](images/example-burning-ship.jpg)

## Choosing a formula

Click a formula in the browser. Its parameters are reset to their defaults, the view jumps to
the formula's suggested region and the iteration limit to its suggestion; colors and image size
stay. Ctrl+Z goes back. The search field filters by name, family or id.

## Parameters

The *Formula* section lists the formula's parameters with their own controls: numbers (slider
or drag field), complex numbers (real and imaginary part), switches, choices and functions
(sin, exp, sqr, …). The reset button of the section restores the defaults.

**Iterations** limits the steps per point. Points that never escape within the limit belong to
the set (colored by *Inside*). Deep views and thin structures need more iterations; more take
longer.

![Newton's method for z⁵ = 1 with relaxation (1.2, 0.3)](images/example-newton.jpg)

## Formula files

RFract reads formulas in three languages:

- **UFM syntax** (`.ufm`): sections `init:`, `loop:`, `bailout:` and `default:` (parameters
  with `param … endparam`, function parameters with `func … endfunc`). `|z|` is the squared
  magnitude, `#pixel` the point, `@name` a parameter. RFract adds `#ismand` (true in Mandelbrot
  mode) and `#c` (the point in Mandelbrot mode, the seed in Julia mode): formulas that use them
  get the Julia switch. RFract's built-in formulas are written this way
  (`formulas/rfract/rfract.ufm` in the source code).
- **FractalPark FRM-like v1** (`.frm` with the `; @language: frm-like/1` header): the language
  of the bundled FractalPark formulas.
- **FractInt formulas** (`.frm`): `name { init : loop, bailout }`, with FractInt's predefined
  names and functions. A symmetry after the name (`Mandelbrot(XAXIS)`) is ignored.

### Your formula folders

*File › Preferences › Formula folders* lists folders with your own formula files: `.ufm` and
`.frm` (fractals), `.ucl` (colorings) and `.uxf` (transformations), subfolders included. RFract
reads the folders in the background when it starts and when the list changes (*Rescan* after
you changed files); the status bar reports how many formulas it found.

The formulas appear in the browser under *User formulas*, one group per file; user colorings
and transformations appear in the coloring picker and in *Add a transformation*. A formula
compiles when you pick it; if it does not compile, the view shows why and the browser hides it
afterwards. Documents store user formulas as `user:<file path in the folder>:<entry name>`, so
a document opens on another computer as long as the same file lies in one of its formula
folders.

The command line checks formula files and reports what does not compile yet:

```
rfr-cli formulas --check path/to/formulas
```

### What the UFM support covers

- Types `bool`, `int`, `float`, `complex` and `color`; declared variables keep their type,
  undeclared ones take the widest type assigned to them.
- Sections `global:`, `init:`, `loop:`, `bailout:`, `final:`, `transform:` and `default:`;
  `if`/`elseif`/`else`, `while`, `repeat … until`; several statements on one line separated
  by commas; chained assignments (`z = c = #pixel`).
- Parameters of all simple kinds (bool, int, enumerations, float, complex, color, function);
  undeclared `@name` parameters become complex parameters, undeclared `@fn` function
  parameters default to `ident`. Assigning to a parameter (`@p = …`) turns it into a
  variable that starts with the parameter's value.
- Arrays: `float a[10]`, `complex b[4, 4]`, dynamic arrays with `setLength(a, n)` and
  `length(a)`. Each sample keeps its own arrays (at most 4096 elements; dynamic arrays get
  2048). Arrays as large as the image (summing over all pixels) are not supported. Indices
  outside the array are clamped to its ends.
- Colors: `rgb`, `rgba`, `hsl`, `hsla` (hue 0 to 6), `red`, `green`, `blue`, `alpha`, `hue`,
  `sat`, `lum`, `blend`, `compose`, `gradient` (the coloring's gradient) and the merge
  functions (`mergenormal`, `mergemultiply`, `mergescreen`, … `mergeluminance`). Colors are
  sRGB values from 0 to 1 with straight alpha. A coloring formula that writes `#color` colors
  the region directly; the gradient then only serves `gradient()`.
- Symbols `#pixel`, `#z`, `#c`, `#zprev`, `#numiter`, `#maxiter`, `#ismand`, `#center`,
  `#magn`, `#x`, `#y`, `#width`, `#height`, `#screenpixel`, `#screenmax`, `#angle`,
  `#random` (a new random complex number per use) and `#randomrange`; `random(seed)` gives
  the next pseudo-random integer after `seed`.
- FractInt syntax inside UFM files (`init : loop, bailout`), its predefined names (`pixel`,
  `p1`…`p6`, `fn1`…`fn4`, `whitesq`, `scrnpix`, `maxit`, `lastsqr`, `rand`, …) and the
  preprocessor (`$define`, `$ifdef`, `$ifndef`, `$else`, `$endif`).

!!! note
    Not supported yet: classes (`class`, `import`, `new`), user functions, image parameters
    and `.upr` parameter files. The check lists what fails by frequency.
