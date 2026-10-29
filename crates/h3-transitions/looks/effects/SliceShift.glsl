// Author: H3 Studio
// License: H3 Studio
// Description: The picture is cut into strips that slide in opposite directions
// Category: Glitch
// Tags: effect, glitch, slice, strips, shift

// @param How many strips
uniform int slices; // = 8 (2 .. 32)
// @param How far a strip slides
uniform float amount; // = 0.08 (0.0 .. 0.4)

vec4 effect(vec2 uv) {
  float dir = mod(floor(uv.y * float(slices)), 2.0) * 2.0 - 1.0;
  return getColor(fold(vec2(uv.x + dir * amount * sin(PI * progress), uv.y)));
}
