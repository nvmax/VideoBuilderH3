// Author: H3 Studio
// License: H3 Studio
// Description: Every colour inverted
// Category: Color
// Tags: effect, color, negative, invert

vec4 effect(vec2 uv) { return vec4(vec3(1.0) - getColor(uv).rgb, 1.0); }
