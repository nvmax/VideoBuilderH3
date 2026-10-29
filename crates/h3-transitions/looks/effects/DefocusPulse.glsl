// Author: H3 Studio
// License: H3 Studio
// Description: The picture drifts out of focus and back
// Category: Blur
// Tags: effect, blur, blur, defocus, pulse

// @param How far out of focus
uniform float radius; // = 0.03 (0.0 .. 0.08)

vec4 effect(vec2 uv) { return blurDisc(uv, radius * sin(PI * progress)); }
