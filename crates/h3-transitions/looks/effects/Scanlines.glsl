// Author: H3 Studio
// License: H3 Studio
// Description: Thin dark lines across the picture
// Category: Retro
// Tags: effect, retro, scanlines, lines, tv

// @param How many lines
uniform float lines; // = 180.0 (40.0 .. 400.0)
// @param How dark the lines are
uniform float strength; // = 0.4 (0.0 .. 1.0)

vec4 effect(vec2 uv) {
  return vec4(getColor(uv).rgb * (1.0 - strength * (0.5 - 0.5 * sin(uv.y * lines * PI * 2.0))), 1.0);
}
