// Author: H3 Studio
// License: H3 Studio
// Description: The picture stretches tall and snaps back
// Category: Distort
// Tags: effect, distort, stretch, squash, elastic

// @param How far it stretches
uniform float amount; // = 0.4 (0.0 .. 1.0)

vec4 effect(vec2 uv) {
  float s = 1.0 + amount * sin(PI * progress);
  return getColor(fold(vec2(uv.x, 0.5 + (uv.y - 0.5) / s)));
}
