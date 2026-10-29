// Author: H3 Studio
// License: H3 Studio
// Description: The picture drips downwards as it melts
// Category: Distort
// Tags: effect, distort, melt, drip, liquid

// @param How far it melts
uniform float amount; // = 0.12 (0.0 .. 0.4)
// @param How many drips
uniform float frequency; // = 8.0 (1.0 .. 30.0)

vec4 effect(vec2 uv) {
  float n = vnoise(vec2(uv.x * frequency + 5.0, 3.0));
  return getColor(fold(vec2(uv.x, uv.y + n * amount * ease(progress))));
}
