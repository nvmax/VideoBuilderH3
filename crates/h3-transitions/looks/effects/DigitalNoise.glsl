// Author: H3 Studio
// License: H3 Studio
// Description: Blocks of random colour flicker over the picture
// Category: Glitch
// Tags: effect, glitch, digital, noise, blocks

// @param How big a block is
uniform float size; // = 0.04 (0.01 .. 0.2)
// @param How many blocks show noise
uniform float density; // = 0.3 (0.0 .. 1.0)

vec4 effect(vec2 uv) {
  vec2 cell = floor(uv * vec2(ratio, 1.0) / size);
  float t = floor(time * 12.0);
  float on = step(1.0 - density * 0.3, hash21(cell + t * 31.0));
  vec3 n = vec3(hash21(cell + 5.0 + t), hash21(cell + 9.0 + t), hash21(cell + 13.0 + t));
  return vec4(mix(getColor(uv).rgb, n, on), 1.0);
}
