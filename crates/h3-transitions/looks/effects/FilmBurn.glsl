// Author: H3 Studio
// License: H3 Studio
// Description: Hot orange film burn flickers in from the edge
// Category: Retro
// Tags: effect, retro, film burn, orange, edge

// @param How strong the burn is
uniform float amount; // = 0.9 (0.0 .. 1.5)

vec4 effect(vec2 uv) {
  vec3 c = getColor(uv).rgb;
  float t = floor(time * 14.0);
  float edge = exp(-uv.x * (3.0 + 4.0 * hash11(t))) * (0.4 + 0.6 * hash11(t + 5.0));
  edge += exp(-(1.0 - uv.y) * 6.0) * 0.4 * hash11(t + 11.0);
  vec3 burn = mix(vec3(1.0, 0.25, 0.05), vec3(1.0, 0.85, 0.3), edge);
  return vec4(vec3(1.0) - (vec3(1.0) - c) * (vec3(1.0) - burn * edge * amount), 1.0);
}
