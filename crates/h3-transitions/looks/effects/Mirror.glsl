// Author: H3 Studio
// License: H3 Studio
// Description: One half of the picture is mirrored onto the other
// Category: Distort
// Tags: effect, distort, mirror, reflect, symmetry

// @param 0 mirrors left onto right, 1 top onto bottom, 2 both
uniform int mode; // = 0 (0 .. 2)

vec4 effect(vec2 uv) {
  vec2 p = uv;
  if (mode == 0 || mode == 2) p.x = 0.5 - abs(uv.x - 0.5);
  if (mode == 1 || mode == 2) p.y = 0.5 - abs(uv.y - 0.5);
  return getColor(p);
}
