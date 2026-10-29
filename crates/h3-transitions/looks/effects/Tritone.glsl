// Author: H3 Studio
// License: H3 Studio
// Description: Three colours: darks, middle tones and lights
// Category: Color
// Tags: effect, color, tritone, three colour, gradient

// @param The darks
uniform vec3 shadow; // = vec3(0.05, 0.02, 0.25) color
// @param The middle tones
uniform vec3 middle; // = vec3(0.75, 0.15, 0.45) color
// @param The lights
uniform vec3 light; // = vec3(1.0, 0.85, 0.5) color

vec4 effect(vec2 uv) {
  float l = luma(getColor(uv).rgb);
  vec3 c = l < 0.5 ? mix(shadow, middle, l * 2.0) : mix(middle, light, l * 2.0 - 1.0);
  return vec4(c, 1.0);
}
