// Author: H3 Studio
// License: H3 Studio
// Description: Darkens the corners
// Category: Light
// Tags: effect, light, vignette, dark, edge

// @param How dark the corners get
uniform float amount; // = 0.5 (0.0 .. 1.0)
// @param How soft the edge is
uniform float softness; // = 0.5 (0.05 .. 1.0)

vec4 effect(vec2 uv) {
  float d = length(centred(uv));
  return vec4(getColor(uv).rgb * (1.0 - amount * smoothstep(1.0 - softness, 1.0, d * 1.2)), 1.0);
}
