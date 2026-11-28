--- Formula gallery
--- Renders every fractal formula of a set at its own view into the folder "gallery" (next to the document).
--@param set choice rfract|fractint|fractalpark  The formula set
--@param count integer 24 1 1000                 At most this many images
--@param size integer 320 64 2048                Width of each image in pixels

local n = 0
for _, f in ipairs(rfr.formulas("fractal")) do
  if n >= params.count then break end
  if f.id:sub(1, #params.set + 1) == params.set .. ":" then
    n = n + 1
    local d = rfr.document(f.id)
    local name = (f.title:gsub("[^%w%s%-]", ""))
    local file = string.format("gallery/%03d %s.png", n, name)
    local ok, e = pcall(rfr.render, d, file, { width = params.size, height = math.floor(params.size * 9 / 16), samples = 8 })
    print(ok and file or (f.id .. ": " .. tostring(e)))
  end
end
print(n .. " formulas")
