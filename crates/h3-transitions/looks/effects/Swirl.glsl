// Author: H3 Studio
// License: H3 Studio
// Description: The picture twists around its middle
// Category: Distort
// Tags: effect, distort, swirl, twist, spin

// @param How far it twists, in degrees
uniform float angle; // = 140.0 (-360.0 .. 360.0)
// @param How far the twist reaches
uniform float radius; // = 0.6 (0.1 .. 1.5)
// @param The middle of the twist
uniform vec2 center; // = vec2(0.5, 0.5)

vec4 effect(vec2 uv) {
  vec2 p = (uv - center) * vec2(ratio, 1.0);
  float a = radians(angle) * smoothstep(radius, 0.0, length(p));
  return getColor(fold(rot2(p, a) / vec2(ratio, 1.0) + center));
}
