// Author: H3 Studio
// License: H3 Studio
// Description: Lens colour fringes that grow towards the corners
// Category: Glitch
// Tags: effect, glitch, lens, colour, fringe

// @param How strong the fringe is
uniform float amount; // = 0.03 (0.0 .. 0.15)

vec4 effect(vec2 uv) {
  vec2 d = uv - vec2(0.5);
  return vec4(getColor(fold(vec2(0.5) + d * (1.0 + amount))).r, getColor(uv).g, getColor(fold(vec2(0.5) + d * (1.0 - amount))).b, 1.0);
}
