// Author: H3 Studio
// License: H3 Studio
// Description: A blur that turns around a point
// Category: Blur
// Tags: effect, blur, blur, spin, rotational

// @param How far the blur turns, in degrees
uniform float angle; // = 8.0 (0.0 .. 60.0)
// @param The centre of the turn
uniform vec2 center; // = vec2(0.5, 0.5)

vec4 effect(vec2 uv) {
  vec2 p = (uv - center) * vec2(ratio, 1.0);
  vec4 sum = vec4(0.0);
  for (int i = 0; i < 14; i++) {
    float a = radians(angle) * (float(i) / 13.0 - 0.5);
    sum += getColor(fold(rot2(p, a) / vec2(ratio, 1.0) + center));
  }
  return sum / 14.0;
}
