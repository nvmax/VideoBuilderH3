// Author: H3 Studio
// License: H3 Studio
// Description: Rings spread out from a point like a drop in water
// Category: Distort
// Tags: effect, distort, ripple, water, rings

// @param How tall the waves are
uniform float amount; // = 0.02 (0.0 .. 0.06)
// @param How many rings
uniform float frequency; // = 40.0 (5.0 .. 120.0)
// @param How fast they travel
uniform float speed; // = 4.0 (0.0 .. 15.0)
// @param Where the rings start
uniform vec2 center; // = vec2(0.5, 0.5)

vec4 effect(vec2 uv) {
  vec2 p = (uv - center) * vec2(ratio, 1.0);
  float d = length(p);
  float w = sin(d * frequency - time * speed) * amount / (1.0 + d * 3.0);
  return getColor(fold((p + p / max(d, 0.0001) * w) / vec2(ratio, 1.0) + center));
}
