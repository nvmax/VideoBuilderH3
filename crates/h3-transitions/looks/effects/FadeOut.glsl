// Author: H3 Studio
// License: H3 Studio
// Description: Fades out to a colour
// Category: Light
// Tags: effect, light, fade, out, black

// @param The colour it fades out to
uniform vec3 color; // = vec3(0.0, 0.0, 0.0) color

vec4 effect(vec2 uv) { return vec4(mix(getColor(uv).rgb, color, ease(progress)), 1.0); }
