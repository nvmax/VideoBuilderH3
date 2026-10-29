// Author: H3 Studio
// License: H3 Studio
// Description: A burst of colour covers the picture and fades away over the next one
// Tags: flash, fade, light, color, trending, original

// @param The colour of the flash
uniform vec4 color; // = vec4(1.0, 1.0, 1.0, 1.0) color
// @param How long the flash stays at full strength
uniform float hold; // = 0.5 (0.0 .. 1.0)

vec4 transition(vec2 uv) {
  float t = progress;
  float k = pow(min(smoothstep(0.0, 0.5, t), 1.0 - smoothstep(0.5, 1.0, t)), mix(1.6, 0.4, hold));
  vec4 base = mix(getFromColor(uv), getToColor(uv), step(0.5, t));
  return vec4(mix(base.rgb, color.rgb, k), 1.0);
}
