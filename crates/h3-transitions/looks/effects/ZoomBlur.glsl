// Author: H3 Studio
// License: H3 Studio
// Description: The picture streaks outwards from a point
// Category: Blur
// Tags: effect, blur, blur, zoom, radial

// @param How far it streaks
uniform float amount; // = 0.12 (0.0 .. 0.5)
// @param Where the streaks start
uniform vec2 center; // = vec2(0.5, 0.5)

vec4 effect(vec2 uv) { return radialBlur(uv, center, amount); }
