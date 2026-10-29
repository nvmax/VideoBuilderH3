// Author: H3 Studio
// License: H3 Studio
// Description: Only the edges of the picture, as bright lines on black
// Category: Stylize
// Tags: effect, stylize, edges, outline, lines

// @param How wide the lines are
uniform float width; // = 0.002 (0.0005 .. 0.01)
// @param How strong the lines are
uniform float gain; // = 3.0 (0.5 .. 10.0)
// @param 0 is white lines, 1 takes the picture's colour
uniform float colored; // = 0.0 (0.0 .. 1.0)

vec4 effect(vec2 uv) {
  float e = clamp(edgeAt(uv, width) * gain, 0.0, 1.0);
  return vec4(e * mix(vec3(1.0), getColor(uv).rgb * 1.6, colored), 1.0);
}
