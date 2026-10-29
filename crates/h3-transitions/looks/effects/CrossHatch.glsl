// Author: H3 Studio
// License: H3 Studio
// Description: Pen hatching: darker parts get more lines
// Category: Stylize
// Tags: effect, stylize, hatch, pen, drawing

// @param How far apart the lines are
uniform float spacing; // = 0.012 (0.004 .. 0.04)

vec4 effect(vec2 uv) {
  vec3 c = getColor(uv).rgb;
  float l = luma(c);
  vec2 p = uv * vec2(ratio, 1.0) / spacing;
  float h = 1.0;
  if (l < 0.92) h *= step(0.12, abs(fract((p.x + p.y) * 0.5) - 0.5));
  if (l < 0.72) h *= step(0.12, abs(fract((p.x - p.y) * 0.5) - 0.5));
  if (l < 0.52) h *= step(0.12, abs(fract(p.y * 0.5) - 0.5));
  if (l < 0.32) h *= step(0.12, abs(fract(p.x * 0.5) - 0.5));
  return vec4(vec3(h) * 0.96, 1.0);
}
