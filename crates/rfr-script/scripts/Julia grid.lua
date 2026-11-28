--- Julia grid
--- Renders the Julia sets of a grid of seeds along the top half of the Mandelbrot set into the folder "julia-grid" (next to the document).
--@param count integer 4 2 12      Seeds per row and column
--@param size integer 256 64 2048  Side of each image in pixels
--@param samples integer 16 1 256  Samples per pixel

local base = rfr.document("rfract:Mandelbrot")
base.layers[1].julia = true
base.view = { center = { "0", "0" }, radius = 1.6, rotation = 0 }
local n = params.count
for j = 0, n - 1 do
  for i = 0, n - 1 do
    local re = -1.6 + 2.0 * i / (n - 1)
    local im = 1.1 * j / (n - 1)
    local d = rfr.copy(base)
    d.layers[1].seed = { re, im }
    local file = string.format("julia-grid/julia_%02d_%02d.png", j + 1, i + 1)
    rfr.render(d, file, { width = params.size, height = params.size, samples = params.samples })
    print(string.format("%s  seed %.3f %+.3fi", file, re, im))
  end
end
