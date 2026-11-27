# Formulas

A formula describes one iteration step: how *z* changes from one step to the next, where it
starts and when it has escaped (the bailout condition). RFract ships 566 formulas:

- **RFract** (23): Mandelbrot (integer powers), Multibrot (real powers), Burning Ship, Tricorn,
  Celtic, Buffalo, Perpendicular Mandelbrot, Phoenix, Lambda, Magnet I and II, Newton, Nova,
  Spider, Manowar, three Barnsley formulas, Lyapunov (Markus–Lyapunov sequences), Truchet
  tiles, Sierpinski carpet, Root finding (polynomial) (Newton, Halley or Schröder steps for
  any polynomial up to degree 6, with relaxation and Nova) and Root finding (function)
  (Newton, Halley, Schröder or Householder steps for f(z) = a·fn1(z)^p + b·fn2(z) + c with
  two function parameters; its derivatives come from `deriv`, so a copy of it finds the
  roots of any function you write).
- **Attractors** (9): strange attractors for density layers (see *Layers*).
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

The browser's *RFract › FractInt types* group holds FractInt's escape-time fractal types
(lambda, the fn types, phoenix, Barnsley, Man-o-war, spider, Marks, Halley, complex Newton,
popcorn Julia, tetrate, unity, Lyapunov and more) with FractInt's parameter layout, so `.par`
files carry over; mandel, julia, magnet and newton use RFract's own formulas. Color the
*lyapunov* type with the coloring *Value of z*.

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

### Formula editor

*Source* next to a formula's name (and below the coloring picker) opens the formula editor.
It shows the formula's entry and checks it while you type: the line above the text says
whether it compiles or names the line and the problem. **Save** (or Ctrl+S in the editor)
writes the entry back into its file and the view shows the change; **Revert** goes back to the
saved text.

Built-in formulas open read-only. *Copy to my formulas* copies one into `my-formulas.ufm`
(`.ucl`, `.uxf`) in your first formula folder, where you can change it; a copied fractal
formula replaces the original on the selected layer. The **+** button above the formula list
creates a new fractal formula, coloring or transformation from a small template in the same
file and opens it.

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
- Functions of the formula: `complex func f(complex w, int n) … endfunc` (or `func p()`
  without a value) in any section, called like built-in functions; arrays are passed by
  reference. `complex b[n] = a` and `b = a` copy array `a`. RFract builds every call into the formula, as it does with class methods.
- Assignments inside expressions: `(x = sqrt(y)) < 1`, `(a = b = 0)`, `abs(r = random(r))`,
  `until (i = i + 1) >= 10` assign and give the assigned value. A single `=` compares when
  its left side cannot be assigned or a text follows (`(@mode = "Julia")`).
- Derivatives: `deriv(e, x)` is the derivative of the real or complex expression `e` with
  respect to the variable `x` (for complex functions the complex derivative), built by
  RFract from the expression. Variables that were assigned earlier in the same section are
  followed, so `f = z^3 - 1` and then `d = deriv(f, z)` and `dd = deriv(d, z)` give the
  first and second derivative. Variables that change under a condition or in a loop in
  between, parameters and rounding functions count as constants. Root finding formulas
  (Newton, Halley, …) need no hand-written derivatives this way.
- Colors: `rgb`, `rgba`, `hsl`, `hsla` (hue 0 to 6), `red`, `green`, `blue`, `alpha`, `hue`,
  `sat`, `lum`, `blend`, `compose`, `gradient` (the coloring's gradient) and the merge
  functions (`mergenormal`, `mergemultiply`, `mergescreen`, … `mergeluminance`). Colors are
  sRGB values from 0 to 1 with straight alpha. `+`, `-`, `*` and `/` work channel by channel
  on colors (`c1 * t + c2 * (1 - t)`); a number counts for all four channels. A coloring
  formula that writes `#color` colors the region directly; the gradient then only serves
  `gradient()`.
- Noise: `noise(p)` (value noise, 0 to 1), `perlin(p)` (gradient noise, about −1 to 1) and
  `voronoi(p)` (distance to the nearest of random points, one per unit cell, 0 to about 1)
  of a complex point `p`; scale `p` for finer or coarser patterns.
- Symbols `#pixel`, `#z`, `#c`, `#zprev`, `#numiter`, `#maxiter`, `#ismand`, `#center`,
  `#magn`, `#x`, `#y`, `#width`, `#height`, `#screenpixel`, `#screenmax`, `#angle`,
  `#random` (a new random complex number per use) and `#randomrange`; `random(seed)` gives
  the next pseudo-random integer after `seed`.
- FractInt syntax inside UFM files (`init : loop, bailout`), its predefined names (`pixel`,
  `p1`…`p6`, `fn1`…`fn4`, `whitesq`, `scrnpix`, `maxit`, `lastsqr`, `rand`, …) and the
  preprocessor (`$define`, `$ifdef`, `$ifndef`, `$else`, `$endif`).

### Classes and plugins

Formulas can use classes from class libraries (`.ulb` files in your formula folders):
`import "common.ulb"`, `Formula f = new @formulaClass(this)`, methods (`f.Iterate(#z)`),
inheritance with base class calls (`Formula.Init(pz)`), static methods, fields and arrays in
objects, and object parameters (plugins): `Formula param formulaClass … endparam` lets you pick
any class derived from `Formula` in the inspector, and the parameters of the chosen class
appear indented below it. RFract brings its own versions of the most used standard classes
(Mandelbrot, Julia, Pixel, Newton, Smooth, Gradient, Default, Orbit Traps and Direct Orbit
Traps with a closest-point trap mode and 18 trap shapes, Exponential Smoothing, Distance
Estimator, Height Transfer); they build on `common.ulb`, which must be in a formula folder.

RFract compiles objects away: every `new` becomes a set of variables and every method call is
built into the formula, so class formulas render on the graphics card like any other.
Changing a plugin compiles the formula again.

Arrays of objects (`TrapShape m_Elements[10]`) have a size known while compiling, or one set
with `setLength` from an int parameter that has a maximum (the array gets room for the
maximum). Counting loops that make or use objects (`int j = 0`, `while j < 10`, `j = j + 1`)
are unrolled, so objects made inside them stay alive; a loop up to such a parameter
(`while j < @count`) unrolls to its maximum and checks the condition while rendering.

When the object a variable or an array element refers to depends on the rendering (a
condition on a parameter, an index from a parameter, a method that returns different objects
in different branches), the formula keeps all candidates and picks the right one while
rendering. Each method call on such a variable is built in once per candidate.

### Image parameters

`Image param name … endparam` lets you choose an image file (PNG or JPEG) in the inspector.
Formulas read it through an image object: `Image img = new @name`, then `img.getColor(z)`
(the whole image covers the square from −1−1i to 1+1i, the top row at +1i; transparent
outside), `img.getPixel(x, y)` (pixel coordinates from the top left), `img.getWidth()`,
`img.getHeight()` and `img.getEmpty()` (no image chosen or the file cannot be read). Class
libraries use them through their image classes (`ImageImport` and its relatives). Images
larger than 4096 × 4096 pixels are scaled down; they render on the graphics card like
everything else.

!!! note
    Not supported yet: object arrays sized by a value without a known maximum (such as the
    large object lists of ray tracers), a loop that changes which object a variable refers to,
    and recursion. The check lists what fails by frequency.
