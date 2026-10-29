// Author: H3 Studio
// License: H3 Studio
// Description: Cinema bars across the top and bottom
// Category: Frame
// Tags: effect, frame, letterbox, bars, cinema

// @param How tall each bar is (the picture's height is 1)
uniform float bar; // = 0.12 (0.0 .. 0.4)
// @param The colour of the bars
uniform vec3 color; // = vec3(0.0, 0.0, 0.0) color

vec4 effect(vec2 uv) {
  float inBars = step(uv.y, bar) + step(1.0 - bar, uv.y);
  return vec4(mix(getColor(uv).rgb, color, clamp(inBars, 0.0, 1.0)), 1.0);
}
