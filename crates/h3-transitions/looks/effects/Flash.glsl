// Author: H3 Studio
// License: H3 Studio
// Description: A flash of light that peaks in the middle of the block
// Category: Light
// Tags: effect, light, flash, light, hit

// @param The colour of the flash
uniform vec3 color; // = vec3(1.0, 1.0, 1.0) color
// @param How strong the flash gets
uniform float peak; // = 0.9 (0.0 .. 1.0)
// @param How sharp the peak is
uniform float sharpness; // = 3.0 (0.5 .. 12.0)

vec4 effect(vec2 uv) {
  float e = pow(sin(PI * progress), sharpness) * peak;
  return vec4(mix(getColor(uv).rgb, color, e), 1.0);
}
