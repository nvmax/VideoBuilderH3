// Author: H3 Studio
// License: H3 Studio
// Description: The red, green and blue of the picture hand over to the next one one after the other, split apart from the centre
// Tags: prism, rgb, chromatic, fade, trending, original

// @param How far the colours split apart
uniform float spread; // = 0.04 (0.0 .. 0.2)

vec4 transition(vec2 uv) {
  float t = progress;
  float s = sin(PI * t) * spread;
  vec2 dir = normalize(uv - 0.5 + vec2(0.0001));
  vec2 pr = fold(uv + dir * s);
  vec2 pb = fold(uv - dir * s);
  float r = mix(getFromColor(pr).r, getToColor(pr).r, smoothstep(0.0, 0.62, t));
  float g = mix(getFromColor(uv).g, getToColor(uv).g, smoothstep(0.19, 0.81, t));
  float b = mix(getFromColor(pb).b, getToColor(pb).b, smoothstep(0.38, 1.0, t));
  return vec4(r, g, b, 1.0);
}
