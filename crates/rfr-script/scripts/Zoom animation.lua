--- Zoom animation
--- Keyframes that zoom into the middle of the current view, one keyframe per step (replaces the animation).
--@param steps integer 6 1 40          Keyframes after the first
--@param factor number 0.1 0.001 0.9   Zoom of each step (0.1: ten times closer)
--@param frames integer 60 1 600       Frames from one keyframe to the next
--@param turn number 0 -180 180        Rotation of the view per step in degrees

doc.animation = nil
rfr.add_keyframe(doc, params.frames)
for i = 1, params.steps do
  rfr.zoom(doc, params.factor)
  doc.view.rotation = (doc.view.rotation or 0) + params.turn
  rfr.add_keyframe(doc, params.frames)
end
print(string.format("%d keyframes, %d frames", params.steps + 1, params.steps * params.frames + 1))
