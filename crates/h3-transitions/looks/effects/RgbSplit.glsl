// Author: H3 Studio
// License: H3 Studio
// Description: The red and blue channels slide apart in a direction you choose
// Category: Glitch
// Tags: effect, glitch, rgb, split, colour

// @param How far the channels are pulled apart
uniform float amount; // = 0.015 (0.0 .. 0.1)
// @param The direction, in degrees
uniform float angle; // = 0.0 (0.0 .. 360.0)

vec4 effect(vec2 uv) {
  float a = radians(angle);
  vec2 o = vec2(cos(a), sin(a)) * amount / vec2(ratio, 1.0);
  return vec4(getColor(fold(uv + o)).r, getColor(uv).g, getColor(fold(uv - o)).b, 1.0);
}
