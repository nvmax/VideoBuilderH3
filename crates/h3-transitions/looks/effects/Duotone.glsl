// Author: H3 Studio
// License: H3 Studio
// Description: Two colours: one for the darks and one for the lights
// Category: Color
// Tags: effect, color, duotone, two colour

// @param The colour of the dark parts
uniform vec3 shadow; // = vec3(0.08, 0.05, 0.35) color
// @param The colour of the light parts
uniform vec3 light; // = vec3(1.0, 0.45, 0.55) color

vec4 effect(vec2 uv) { return vec4(mix(shadow, light, luma(getColor(uv).rgb)), 1.0); }
