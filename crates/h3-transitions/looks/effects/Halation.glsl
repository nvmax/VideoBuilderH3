// Author: H3 Studio
// License: H3 Studio
// Description: A red-orange halo around bright edges, like film
// Category: Light
// Tags: effect, light, halation, film, red

// @param How bright a part must be to halo
uniform float threshold; // = 0.7 (0.0 .. 1.0)
// @param How strong the halo is
uniform float amount; // = 0.8 (0.0 .. 2.0)
// @param How far it spreads
uniform float radius; // = 0.04 (0.01 .. 0.12)

vec4 effect(vec2 uv) {
  vec3 c = getColor(uv).rgb;
  float g = 0.0;
  for (int i = 0; i < 20; i++) {
    float a = float(i) * 2.399963;
    float r = sqrt((float(i) + 0.5) / 20.0) * radius;
    g += max(luma(getColor(fold(uv + vec2(cos(a), sin(a)) * r / vec2(ratio, 1.0))).rgb) - threshold, 0.0);
  }
  return vec4(c + g / 20.0 * amount * 4.0 * vec3(1.0, 0.32, 0.12), 1.0);
}
