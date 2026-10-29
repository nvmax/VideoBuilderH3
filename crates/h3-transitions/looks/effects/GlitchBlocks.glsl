// Author: H3 Studio
// License: H3 Studio
// Description: Rows of the picture jump sideways with a colour split
// Category: Glitch
// Tags: effect, glitch, glitch, digital, blocks

// @param How many rows the picture is cut into
uniform int rows; // = 18 (4 .. 60)
// @param How far a row jumps
uniform float amount; // = 0.08 (0.0 .. 0.3)
// @param How many rows glitch at once
uniform float density; // = 0.25 (0.0 .. 1.0)
// @param How often the pattern changes
uniform float speed; // = 10.0 (1.0 .. 30.0)

vec4 effect(vec2 uv) {
  float row = floor(uv.y * float(rows));
  float t = floor(time * speed);
  float on = step(1.0 - density, hash21(vec2(row, t)));
  float shift = (hash21(vec2(row + 41.0, t)) - 0.5) * 2.0 * amount * on;
  vec2 p = vec2(uv.x + shift, uv.y);
  float split = on * amount * 0.25;
  return vec4(getColor(fold(p + vec2(split, 0.0))).r, getColor(fold(p)).g, getColor(fold(p - vec2(split, 0.0))).b, 1.0);
}
