// Author: H3 Studio
// License: H3 Studio
// Description: The colours cycle round the rainbow
// Category: Color
// Tags: effect, color, hue, cycle, rainbow

// @param How fast the colours cycle
uniform float speed; // = 0.4 (-3.0 .. 3.0)

vec4 effect(vec2 uv) {
  vec3 hsv = rgb2hsv(getColor(uv).rgb);
  hsv.x = fract(hsv.x + time * speed);
  return vec4(hsv2rgb(hsv), 1.0);
}
