// Author: H3 Studio
// License: H3 Studio
// Description: Bright tones turn negative, like an overexposed print
// Category: Color
// Tags: effect, color, solarize, negative, print

// @param Where the tones turn
uniform float threshold; // = 0.5 (0.0 .. 1.0)

vec4 effect(vec2 uv) {
  vec3 c = getColor(uv).rgb;
  return vec4(mix(c, vec3(1.0) - c, step(vec3(threshold), c)), 1.0);
}
