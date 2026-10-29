// Author: H3 Studio
// License: H3 Studio
// Description: Seen through frosted glass
// Category: Blur
// Tags: effect, blur, frosted, glass, blur

// @param How rough the glass is
uniform float amount; // = 0.012 (0.0 .. 0.05)

vec4 effect(vec2 uv) {
  vec2 cell = floor(uv * vec2(ratio, 1.0) * 700.0);
  vec4 sum = vec4(0.0);
  for (int i = 0; i < 12; i++) {
    vec2 o = (vec2(hash21(cell + float(i)), hash21(cell + float(i) + 31.0)) - 0.5) * 2.0 * amount;
    sum += getColor(fold(uv + o));
  }
  return sum / 12.0;
}
