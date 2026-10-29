// Author: H3 Studio
// License: H3 Studio
// Description: The picture flashes on and off
// Category: Light
// Tags: effect, light, strobe, flash, rave

// @param Flashes a second
uniform float rate; // = 8.0 (1.0 .. 30.0)
// @param The colour of the flash
uniform vec3 color; // = vec3(1.0, 1.0, 1.0) color
// @param How strong a flash is
uniform float amount; // = 0.8 (0.0 .. 1.0)

vec4 effect(vec2 uv) {
  float on = step(0.5, fract(time * rate));
  return vec4(mix(getColor(uv).rgb, color, on * amount), 1.0);
}
