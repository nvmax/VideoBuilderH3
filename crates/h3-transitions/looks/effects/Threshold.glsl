// Author: H3 Studio
// License: H3 Studio
// Description: Pure black and white with nothing in between
// Category: Color
// Tags: effect, color, threshold, black, white

// @param Where the cut is
uniform float level; // = 0.5 (0.0 .. 1.0)
// @param The dark colour
uniform vec3 dark; // = vec3(0.0, 0.0, 0.0) color
// @param The bright colour
uniform vec3 bright; // = vec3(1.0, 1.0, 1.0) color

vec4 effect(vec2 uv) { return vec4(mix(dark, bright, step(level, luma(getColor(uv).rgb))), 1.0); }
