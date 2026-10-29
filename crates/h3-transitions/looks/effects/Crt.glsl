// Author: H3 Studio
// License: H3 Studio
// Description: An old tube screen: curved glass, scanlines and a colour fringe
// Category: Retro
// Tags: effect, retro, crt, tv, tube, scanlines

// @param How curved the glass is
uniform float curve; // = 0.5 (0.0 .. 1.0)
// @param How many scanlines
uniform float lines; // = 180.0 (40.0 .. 400.0)
// @param How dark the lines are
uniform float scanline; // = 0.4 (0.0 .. 1.0)

vec4 effect(vec2 uv) {
  vec2 p = uv * 2.0 - 1.0;
  p *= 1.0 + curve * dot(p, p) * 0.1;
  vec2 q = p * 0.5 + 0.5;
  float edge = step(0.0, q.x) * step(q.x, 1.0) * step(0.0, q.y) * step(q.y, 1.0);
  float o = 0.0015;
  vec3 c = vec3(getColor(clamp(q + vec2(o, 0.0), 0.0, 1.0)).r, getColor(clamp(q, 0.0, 1.0)).g, getColor(clamp(q - vec2(o, 0.0), 0.0, 1.0)).b);
  c *= 1.0 - scanline * (0.5 - 0.5 * sin(q.y * lines * PI * 2.0));
  c *= 1.0 - 0.35 * smoothstep(0.7, 1.4, length(p));
  return vec4(c * edge, 1.0);
}
