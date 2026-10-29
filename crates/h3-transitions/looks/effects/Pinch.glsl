// Author: H3 Studio
// License: H3 Studio
// Description: The middle of the picture is squeezed inwards
// Category: Distort
// Tags: effect, distort, pinch, squeeze, lens

// @param The middle of the pinch
uniform vec2 center; // = vec2(0.5, 0.5)
// @param How far the pinch reaches
uniform float radius; // = 0.45 (0.05 .. 1.0)
// @param How strong it is
uniform float strength; // = 0.6 (0.0 .. 1.0)

vec4 effect(vec2 uv) {
  vec2 p = (uv - center) * vec2(ratio, 1.0);
  float d = length(p) / radius;
  vec2 q = p * (d < 1.0 ? 1.0 + strength * (1.0 - d * d) : 1.0);
  return getColor(fold(q / vec2(ratio, 1.0) + center));
}
