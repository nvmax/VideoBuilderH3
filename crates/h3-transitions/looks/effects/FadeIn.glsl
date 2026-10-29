// Author: H3 Studio
// License: H3 Studio
// Description: Fades in from a colour
// Category: Light
// Tags: effect, light, fade, in, black

// @param The colour it fades in from
uniform vec3 color; // = vec3(0.0, 0.0, 0.0) color

vec4 effect(vec2 uv) { return vec4(mix(color, getColor(uv).rgb, ease(progress)), 1.0); }
