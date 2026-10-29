// Author: H3 Studio
// License: H3 Studio
// Description: A painted look: detail smoothed into brush-like patches
// Category: Stylize
// Tags: effect, stylize, paint, oil, kuwahara

// @param How big the brush patches are
uniform float radius; // = 0.004 (0.001 .. 0.012)

vec4 effect(vec2 uv) {
  vec2 o = vec2(radius / ratio, radius);
  vec3 best = getColor(uv).rgb;
  float bestVar = 1.0e9;
  for (int q = 0; q < 4; q++) {
    vec2 s = vec2((q == 0 || q == 3) ? -1.0 : 1.0, q < 2 ? -1.0 : 1.0);
    vec3 mean = vec3(0.0);
    float sq = 0.0;
    for (int i = 0; i < 3; i++) {
      for (int j = 0; j < 3; j++) {
        vec3 c = getColor(fold(uv + s * vec2(float(i), float(j)) * o)).rgb;
        mean += c;
        sq += dot(c, c);
      }
    }
    mean /= 9.0;
    float v = sq / 9.0 - dot(mean, mean);
    if (v < bestVar) {
      bestVar = v;
      best = mean;
    }
  }
  return vec4(best, 1.0);
}
