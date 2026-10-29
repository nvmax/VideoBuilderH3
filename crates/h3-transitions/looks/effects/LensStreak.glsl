// Author: H3 Studio
// License: H3 Studio
// Description: A long blue streak across bright lights, like an anamorphic lens
// Category: Light
// Tags: effect, light, lens, streak, anamorphic

// @param How bright a part must be to streak
uniform float threshold; // = 0.7 (0.0 .. 1.0)
// @param How strong the streak is
uniform float amount; // = 1.0 (0.0 .. 2.0)
// @param How long the streak is
uniform float length_; // = 0.25 (0.05 .. 0.6)

vec4 effect(vec2 uv) {
  vec3 c = getColor(uv).rgb;
  vec3 s = vec3(0.0);
  for (int i = 0; i < 24; i++) {
    float t = float(i) / 23.0 - 0.5;
    s += max(getColor(fold(uv + vec2(t * length_, 0.0))).rgb - vec3(threshold), vec3(0.0)) * (1.0 - abs(t) * 1.6);
  }
  return vec4(c + s / 12.0 * amount * vec3(0.35, 0.6, 1.0) * 2.0, 1.0);
}
