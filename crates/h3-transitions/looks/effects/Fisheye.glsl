// Author: H3 Studio
// License: H3 Studio
// Description: A wide angle lens that bends the edges
// Category: Distort
// Tags: effect, distort, fisheye, lens, wide

// @param How much it bends (negative bends the other way)
uniform float amount; // = 0.2 (-1.0 .. 1.5)

vec4 effect(vec2 uv) {
  vec2 p = centred(uv);
  return getColor(fold(uncentred(p * (1.0 + amount * dot(p, p) * 2.0))));
}
