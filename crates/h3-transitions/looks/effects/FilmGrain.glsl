// Author: H3 Studio
// License: H3 Studio
// Description: Film grain over the picture
// Category: Retro
// Tags: effect, retro, grain, film, noise

// @param How strong the grain is
uniform float amount; // = 0.35 (0.0 .. 1.0)
// @param How coarse the grain is
uniform float size; // = 0.0025 (0.001 .. 0.01)

vec4 effect(vec2 uv) {
  vec3 c = getColor(uv).rgb;
  float n = hash21(floor(uv * vec2(ratio, 1.0) / size) + floor(time * 24.0) * 29.0) - 0.5;
  float mid = 1.0 - abs(luma(c) - 0.5) * 1.4;
  return vec4(c + n * amount * 0.5 * mid, 1.0);
}
