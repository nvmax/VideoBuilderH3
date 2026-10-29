// Author: H3 Studio
// License: H3 Studio
// Description: The picture folds into a turning kaleidoscope
// Category: Distort
// Tags: effect, distort, kaleidoscope, mirror, pattern

// @param How many segments
uniform int segments; // = 6 (2 .. 16)
// @param How fast it turns
uniform float spin; // = 0.2 (-2.0 .. 2.0)

vec4 effect(vec2 uv) {
  vec2 p = centred(uv);
  float seg = 2.0 * PI / float(segments);
  float a = mod(atan(p.y, p.x) + time * spin, seg);
  a = abs(a - seg * 0.5);
  return getColor(fold(uncentred(length(p) * vec2(cos(a), sin(a)))));
}
