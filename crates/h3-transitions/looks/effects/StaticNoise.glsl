// Author: H3 Studio
// License: H3 Studio
// Description: TV static over the picture
// Category: Glitch
// Tags: effect, glitch, static, tv, noise

// @param How much static
uniform float amount; // = 0.35 (0.0 .. 1.0)
// @param How coarse the static is
uniform float size; // = 0.004 (0.001 .. 0.02)

vec4 effect(vec2 uv) {
  float n = hash21(floor(uv * vec2(ratio, 1.0) / size) + floor(time * 30.0) * 23.0);
  return vec4(mix(getColor(uv).rgb, vec3(n), amount), 1.0);
}
