--- Layer stack
--- Puts a second fractal over the current document: the same view, merged with a blend mode.
--@param formula choice rfract:BurningShip|rfract:Tricorn|rfract:Celtic|rfract:Newton  The formula of the new layer
--@param merge choice screen|multiply|overlay|difference|softlight  How it merges with the layers below
--@param opacity number 0.6 0 1   Opacity of the new layer

local l = rfr.layer(params.formula)
l.name = params.formula:gsub("^.*:", "")
l.merge = params.merge
l.opacity = params.opacity
table.insert(doc.layers, l)
print(string.format("%d layers", #doc.layers))
