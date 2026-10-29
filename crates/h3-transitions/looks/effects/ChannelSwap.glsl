// Author: H3 Studio
// License: H3 Studio
// Description: The red, green and blue channels swapped round
// Category: Color
// Tags: effect, color, channel, swap, colour

// @param Which swap: 0 red green blue, 1 blue red green, 2 green blue red, 3 red blue green, 4 blue green red, 5 green red blue
uniform int mode; // = 1 (0 .. 5)

vec4 effect(vec2 uv) {
  vec3 c = getColor(uv).rgb;
  vec3 o = c;
  if (mode == 1) o = c.brg;
  if (mode == 2) o = c.gbr;
  if (mode == 3) o = c.rbg;
  if (mode == 4) o = c.bgr;
  if (mode == 5) o = c.grb;
  return vec4(o, 1.0);
}
