// Author: H3 Studio
// License: H3 Studio
// Description: The picture spins and zooms out of the frame with a rotational blur, and the next one spins in
// Tags: spin, rotate, blur, trending, original

// @param How many turns the spin makes
uniform float turns; // = 0.5 (0.0 .. 2.0)
// @param How much the spin smears the picture
uniform float blur; // = 1.0 (0.0 .. 1.0)

vec2 spun(vec2 uv, float angle, float scale) {
  vec2 p = (uv - 0.5) * vec2(ratio, 1.0);
  p = rot2(p, angle) / scale;
  return fold(p / vec2(ratio, 1.0) + 0.5);
}

vec4 transition(vec2 uv) {
  float e = ease(progress);
  float spread = blur * 0.5 * sin(PI * progress);
  float turn = turns * 2.0 * PI;
  vec4 a = vec4(0.0);
  vec4 b = vec4(0.0);
  for (int i = 0; i < 9; i++) {
    float k = float(i) / 8.0 - 0.5;
    a += getFromColor(spun(uv, e * turn + k * spread, 1.0 + 0.5 * e));
    b += getToColor(spun(uv, -(1.0 - e) * turn + k * spread, 1.0 + 0.5 * (1.0 - e)));
  }
  return mix(a / 9.0, b / 9.0, smoothstep(0.4, 0.6, progress));
}
