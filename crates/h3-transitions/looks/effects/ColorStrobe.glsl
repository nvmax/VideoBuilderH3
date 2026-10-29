// Author: H3 Studio
// License: H3 Studio
// Description: The picture flashes red, green and blue in turn
// Category: Color
// Tags: effect, color, colour, strobe, rgb

// @param Colour changes a second
uniform float rate; // = 6.0 (1.0 .. 24.0)
// @param How strong the colour is
uniform float amount; // = 0.6 (0.0 .. 1.0)

vec4 effect(vec2 uv) {
  float k = mod(floor(time * rate), 3.0);
  vec3 tint = k < 0.5 ? vec3(1.0, 0.2, 0.2) : (k < 1.5 ? vec3(0.2, 1.0, 0.2) : vec3(0.2, 0.3, 1.0));
  vec3 c = getColor(uv).rgb;
  return vec4(mix(c, c * tint * 1.6, amount), 1.0);
}
