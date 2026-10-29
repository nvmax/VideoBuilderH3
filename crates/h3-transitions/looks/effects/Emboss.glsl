// Author: H3 Studio
// License: H3 Studio
// Description: The picture as a relief in gray
// Category: Stylize
// Tags: effect, stylize, emboss, relief, gray

// @param How deep the relief is
uniform float depth; // = 2.0 (0.0 .. 6.0)
// @param The direction the light comes from, in degrees
uniform float angle; // = 45.0 (0.0 .. 360.0)

vec4 effect(vec2 uv) {
  float a = radians(angle);
  vec2 o = vec2(cos(a) / ratio, sin(a)) * 0.003;
  float d = lumaAt(uv + o) - lumaAt(uv - o);
  return vec4(vec3(0.5 + d * depth), 1.0);
}
