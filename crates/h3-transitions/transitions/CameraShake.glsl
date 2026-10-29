// Author: H3 Studio
// License: H3 Studio
// Description: The picture shakes with a colour split, harder and harder, and the next one shakes in and settles
// Tags: shake, camera, glitch, trending, original

// @param How hard the picture shakes
uniform float intensity; // = 1.0 (0.0 .. 3.0)

vec4 split3(float from_, vec2 p, float split) {
  vec2 o = vec2(split, 0.0);
  vec4 r = from_ > 0.5 ? getFromColor(fold(p + o)) : getToColor(fold(p + o));
  vec4 g = from_ > 0.5 ? getFromColor(fold(p)) : getToColor(fold(p));
  vec4 b = from_ > 0.5 ? getFromColor(fold(p - o)) : getToColor(fold(p - o));
  return vec4(r.r, g.g, b.b, 1.0);
}

vec4 transition(vec2 uv) {
  float amp = sin(PI * progress) * 0.045 * intensity;
  float tick = floor(progress * 20.0);
  vec2 jitter = vec2(hash11(tick), hash11(tick + 17.0)) - 0.5;
  vec2 p = uv + jitter * 2.0 * amp;
  return mix(split3(1.0, p, amp * 0.6), split3(0.0, p, amp * 0.6), smoothstep(0.42, 0.58, progress));
}
