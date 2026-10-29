// Author: H3 Studio
// License: H3 Studio
// Description: A dreamy glow: the picture mixed with a blurred copy of itself
// Category: Blur
// Tags: effect, blur, soft, dream, glow

// @param How wide the glow is
uniform float radius; // = 0.02 (0.0 .. 0.08)
// @param How much glow
uniform float amount; // = 0.7 (0.0 .. 1.0)

vec4 effect(vec2 uv) {
  vec3 c = getColor(uv).rgb;
  vec3 b = blurDisc(uv, radius).rgb;
  return vec4(mix(c, vec3(1.0) - (vec3(1.0) - c) * (vec3(1.0) - b), amount), 1.0);
}
