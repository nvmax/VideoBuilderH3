// Author: H3 Studio
// License: H3 Studio
// Description: A worn tape: the picture wobbles, smears its colours and loses tracking as a bright bar sweeps over to the next one
// Tags: vhs, retro, glitch, noise, trending, original

// @param How worn the tape is
uniform float intensity; // = 1.0 (0.0 .. 2.0)

vec4 split3(float from_, vec2 p, float split) {
  vec2 o = vec2(split, 0.0);
  vec4 r = from_ > 0.5 ? getFromColor(fold(p + o)) : getToColor(fold(p + o));
  vec4 g = from_ > 0.5 ? getFromColor(fold(p)) : getToColor(fold(p));
  vec4 b = from_ > 0.5 ? getFromColor(fold(p - o)) : getToColor(fold(p - o));
  return vec4(r.r, g.g, b.b, 1.0);
}

vec4 transition(vec2 uv) {
  float t = progress;
  float amt = sin(PI * t) * intensity;
  float wobble = sin(uv.y * 38.0 + t * 60.0) * 0.006 * amt + (vnoise(vec2(uv.y * 8.0, t * 30.0)) - 0.5) * 0.04 * amt;
  float bar = fract(t * 1.7 + 0.2);
  float inBar = smoothstep(0.09, 0.0, abs(uv.y - bar));
  vec2 p = vec2(uv.x + wobble + inBar * 0.06 * amt, uv.y);
  float split = 0.006 * amt + inBar * 0.02 * amt;
  vec4 c = mix(split3(1.0, p, split), split3(0.0, p, split), smoothstep(0.42, 0.58, t));
  float grey = dot(c.rgb, vec3(0.3, 0.59, 0.11));
  c.rgb = mix(c.rgb, vec3(grey), 0.35 * amt) + inBar * 0.25 * amt;
  c.rgb += (hash21(floor(uv * vec2(320.0, 180.0)) + floor(t * 30.0)) - 0.5) * 0.12 * amt;
  return vec4(c.rgb, 1.0);
}
