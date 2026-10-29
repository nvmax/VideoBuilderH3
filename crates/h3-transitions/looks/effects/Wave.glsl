// Author: H3 Studio
// License: H3 Studio
// Description: The picture waves like a flag
// Category: Distort
// Tags: effect, distort, wave, flag, warp

// @param How tall the waves are
uniform float amount; // = 0.015 (0.0 .. 0.08)
// @param How many waves
uniform float frequency; // = 12.0 (1.0 .. 50.0)
// @param How fast they move
uniform float speed; // = 3.0 (0.0 .. 12.0)

vec4 effect(vec2 uv) {
  return getColor(fold(vec2(uv.x + sin(uv.y * frequency + time * speed) * amount, uv.y + sin(uv.x * frequency * 0.7 + time * speed * 1.3) * amount * 0.6)));
}
