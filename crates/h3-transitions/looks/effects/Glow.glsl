// Author: H3 Studio
// License: H3 Studio
// Description: Bright parts glow softly
// Category: Light
// Tags: effect, light, glow, bright, soft

// @param How bright a part must be to glow
uniform float threshold; // = 0.7 (0.0 .. 1.0)
// @param How strong the glow is
uniform float amount; // = 0.6 (0.0 .. 2.0)
// @param How far it spreads
uniform float radius; // = 0.03 (0.005 .. 0.1)

vec4 effect(vec2 uv) {
  vec3 c = getColor(uv).rgb;
  vec3 g = vec3(0.0);
  for (int i = 0; i < 24; i++) {
    float a = float(i) * 2.399963;
    float r = sqrt((float(i) + 0.5) / 24.0) * radius;
    vec3 s = getColor(fold(uv + vec2(cos(a), sin(a)) * r / vec2(ratio, 1.0))).rgb;
    g += max(s - vec3(threshold), vec3(0.0));
  }
  return vec4(c + g / 24.0 * amount / max(1.0 - threshold, 0.05), 1.0);
}
