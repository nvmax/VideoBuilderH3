// Author: H3 Studio
// License: H3 Studio
// Description: A pencil drawing on white paper
// Category: Stylize
// Tags: effect, stylize, pencil, sketch, drawing

// @param How dark the lines are
uniform float gain; // = 4.0 (0.5 .. 12.0)
// @param How much of the picture's colour is washed in
uniform float colored; // = 0.25 (0.0 .. 1.0)

vec4 effect(vec2 uv) {
  float e = clamp(edgeAt(uv, 0.0018) * gain, 0.0, 1.0);
  vec3 paper = mix(vec3(0.97), getColor(uv).rgb, colored);
  float grain = (hash21(floor(uv * vec2(ratio, 1.0) * 600.0)) - 0.5) * 0.06;
  return vec4(paper * (1.0 - e) + grain, 1.0);
}
