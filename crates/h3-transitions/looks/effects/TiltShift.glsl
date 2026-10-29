// Author: H3 Studio
// License: H3 Studio
// Description: Only a band stays sharp, like a miniature
// Category: Blur
// Tags: effect, blur, tilt shift, miniature

// @param Where the sharp band is (0 is the bottom)
uniform float focus; // = 0.5 (0.0 .. 1.0)
// @param How tall the sharp band is
uniform float band; // = 0.12 (0.0 .. 0.5)
// @param How blurred the rest gets
uniform float radius; // = 0.025 (0.0 .. 0.08)

vec4 effect(vec2 uv) {
  float r = smoothstep(band, band + 0.3, abs(uv.y - focus)) * radius;
  return blurDisc(uv, r);
}
