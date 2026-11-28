--- Color variations
--- Renders the current document with its gradient shifted and stretched in steps into the folder "variations" (next to the document).
--@param count integer 6 2 48      Number of images
--@param width integer 640 64 4096 Width of each image in pixels (the height follows the document)

local w, h = doc.image.width, doc.image.height
local height = math.max(1, math.floor(params.width * h / w + 0.5))
for k = 1, params.count do
  local d = rfr.copy(doc)
  for _, l in ipairs(d.layers) do
    if l.outside then
      l.outside.offset = (l.outside.offset or 0) + (k - 1) / params.count
      l.outside.density = (l.outside.density or 1) * (0.75 + 0.5 * (k - 1) / (params.count - 1))
    end
  end
  local file = string.format("variations/colors_%02d.png", k)
  rfr.render(d, file, { width = params.width, height = height })
  print(file)
end
