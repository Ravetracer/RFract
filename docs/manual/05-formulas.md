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

RFract reads formulas in two languages:

- **UFM syntax** (`.ufm`): sections `init:`, `loop:`, `bailout:` and `default:` (parameters
  with `param … endparam`, function parameters with `func … endfunc`). `|z|` is the squared
  magnitude, `#pixel` the point, `@name` a parameter. RFract adds `#ismand` (true in Mandelbrot
  mode) and `#c` (the point in Mandelbrot mode, the seed in Julia mode): formulas that use them
  get the Julia switch. RFract's built-in formulas are written this way
  (`formulas/rfract/rfract.ufm` in the source code).
- **FractalPark FRM-like v1** (`.frm` with the `; @language: frm-like/1` header): the language
  of the bundled FractalPark formulas.

The command line checks formula files and reports what does not compile yet:

```
rfr-cli formulas --check path/to/formulas
```

!!! note
    This version compiles a subset of the UFM language (no classes or coloring formulas yet);
    the check lists the missing features by frequency.
