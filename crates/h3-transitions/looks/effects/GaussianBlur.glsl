// Author: H3 Studio
// License: H3 Studio
// Description: A soft blur over the whole picture
// Category: Blur
// Tags: effect, blur, blur, soft

// @param How wide the blur is
uniform float radius; // = 0.02 (0.0 .. 0.08)

vec4 effect(vec2 uv) { return blurDisc(uv, radius); }
