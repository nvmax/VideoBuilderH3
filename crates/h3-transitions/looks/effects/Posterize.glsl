// Author: H3 Studio
// License: H3 Studio
// Description: Only a few flat levels of each colour
// Category: Color
// Tags: effect, color, posterize, flat, levels

// @param How many levels
uniform int levels; // = 4 (2 .. 16)

vec4 effect(vec2 uv) {
  float n = float(levels);
  return vec4(floor(getColor(uv).rgb * n) / max(n - 1.0, 1.0), 1.0);
}
