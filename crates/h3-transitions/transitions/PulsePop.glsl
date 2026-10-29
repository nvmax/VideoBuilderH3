// Author: H3 Studio
// License: H3 Studio
// Description: The picture pops forward on the beat and the next one drops in at full size
// Tags: zoom, pop, pulse, flash, trending, original

// @param How far the picture pops forward
uniform float scale; // = 0.3 (0.0 .. 1.0)

vec4 transition(vec2 uv) {
  float e = ease(progress);
  float za = 1.0 + scale * e * e;
  float zb = 1.0 + scale * (1.0 - e) * (1.0 - e);
  vec4 a = getFromColor(fold((uv - 0.5) / za + 0.5));
  vec4 b = getToColor(fold((uv - 0.5) / zb + 0.5));
  vec4 c = mix(a, b, smoothstep(0.42, 0.58, progress));
  c.rgb += 0.2 * sin(PI * progress);
  return c;
}
