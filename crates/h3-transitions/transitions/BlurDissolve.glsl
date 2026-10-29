// Author: H3 Studio
// License: H3 Studio
// Description: Both pictures melt out of focus while they dissolve into each other
// Tags: blur, dissolve, soft, trending, original

// @param How blurry the middle of the transition gets
uniform float radius; // = 0.05 (0.0 .. 0.2)

vec4 transition(vec2 uv) {
  float r = sin(PI * progress) * radius;
  vec2 px = vec2(r / ratio, r);
  vec4 a = getFromColor(uv);
  vec4 b = getToColor(uv);
  for (int i = 0; i < 12; i++) {
    float angle = float(i) * 0.5236;
    vec2 o = vec2(cos(angle), sin(angle)) * px;
    a += getFromColor(fold(uv + o)) + getFromColor(fold(uv + o * 0.5));
    b += getToColor(fold(uv + o)) + getToColor(fold(uv + o * 0.5));
  }
  return mix(a / 25.0, b / 25.0, smoothstep(0.3, 0.7, progress));
}
