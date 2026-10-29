// Author: H3 Studio
// License: H3 Studio
// Description: Flat colours with ink outlines
// Category: Stylize
// Tags: effect, stylize, cartoon, comic, ink

// @param How many flat levels of each colour
uniform int levels; // = 5 (2 .. 10)
// @param How strong an edge must be to get an outline
uniform float threshold; // = 0.5 (0.05 .. 1.0)

vec4 effect(vec2 uv) {
  vec3 c = getColor(uv).rgb;
  float n = float(levels);
  vec3 flat_ = floor(c * n + 0.5) / n;
  float ink = step(threshold, edgeAt(uv, 0.0022));
  return vec4(flat_ * (1.0 - ink), 1.0);
}
