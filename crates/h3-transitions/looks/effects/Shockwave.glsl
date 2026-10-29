// Author: H3 Studio
// License: H3 Studio
// Description: A ring of distortion sweeps out from a point
// Category: Distort
// Tags: effect, distort, shockwave, ring, impact

// @param How hard the ring pushes
uniform float amount; // = 0.06 (0.0 .. 0.2)
// @param How wide the ring is
uniform float width; // = 0.1 (0.02 .. 0.4)
// @param Where the shockwave starts
uniform vec2 center; // = vec2(0.5, 0.5)

vec4 effect(vec2 uv) {
  vec2 p = (uv - center) * vec2(ratio, 1.0);
  float d = length(p);
  float ring = smoothstep(width, 0.0, abs(d - progress * 1.3));
  return getColor(fold((p - p / max(d, 0.0001) * ring * amount) / vec2(ratio, 1.0) + center));
}
