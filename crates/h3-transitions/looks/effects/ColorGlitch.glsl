// Author: H3 Studio
// License: H3 Studio
// Description: Patches of the picture shift apart in separate colours
// Category: Glitch
// Tags: effect, glitch, colour, glitch, patches

// @param How far the patches shift
uniform float amount; // = 0.05 (0.0 .. 0.2)
// @param How many patches glitch
uniform float density; // = 0.3 (0.0 .. 1.0)
// @param How often the pattern changes
uniform float speed; // = 8.0 (1.0 .. 24.0)

vec4 effect(vec2 uv) {
  float t = floor(time * speed);
  vec2 cell = floor(vec2(uv.x * 6.0, uv.y * 14.0));
  float on = step(1.0 - density, hash21(cell + t * 7.0));
  vec2 o = vec2(hash21(cell + 3.0 + t) - 0.5, hash21(cell + 8.0 + t) - 0.5) * amount * on;
  return vec4(getColor(fold(uv + o)).r, getColor(fold(uv - o * 0.5)).g, getColor(fold(uv + o.yx)).b, 1.0);
}
