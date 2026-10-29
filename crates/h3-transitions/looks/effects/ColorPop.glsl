// Author: H3 Studio
// License: H3 Studio
// Description: Only one colour stays; everything else turns gray
// Category: Color
// Tags: effect, color, colour pop, isolate, selective

// @param The colour to keep, in degrees round the colour wheel (0 is red, 120 green, 240 blue)
uniform float hue; // = 0.0 (0.0 .. 360.0)
// @param How wide a colour range is kept, in degrees
uniform float range; // = 40.0 (5.0 .. 120.0)

vec4 effect(vec2 uv) {
  vec3 c = getColor(uv).rgb;
  vec3 hsv = rgb2hsv(c);
  float d = abs(hsv.x * 360.0 - hue);
  d = min(d, 360.0 - d);
  float keep = smoothstep(range, range * 0.5, d) * smoothstep(0.08, 0.3, hsv.y);
  return vec4(mix(vec3(luma(c)), c, keep), 1.0);
}
