// Author: H3 Studio
// License: H3 Studio
// Description: The picture is pulled out sideways until it streaks away and the next one snaps back from a stretch
// Tags: stretch, distort, slide, trending, original

// @param How far the picture is stretched
uniform float stretch; // = 6.0 (1.0 .. 20.0)

vec4 transition(vec2 uv) {
  float t = progress;
  float sa = 1.0 + stretch * t * t * t;
  float sb = 1.0 + stretch * (1.0 - t) * (1.0 - t) * (1.0 - t);
  vec4 a = getFromColor(vec2((uv.x - 0.5) / sa + 0.5, uv.y));
  vec4 b = getToColor(vec2((uv.x - 0.5) / sb + 0.5, uv.y));
  return mix(a, b, smoothstep(0.4, 0.6, t));
}
