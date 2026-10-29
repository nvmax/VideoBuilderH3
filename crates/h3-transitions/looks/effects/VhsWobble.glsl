// Author: H3 Studio
// License: H3 Studio
// Description: Wavy lines, colour bleed and noise like a worn tape
// Category: Glitch
// Tags: effect, glitch, vhs, tape, wobble

// @param How worn the tape is
uniform float amount; // = 0.6 (0.0 .. 1.0)

vec4 effect(vec2 uv) {
  float w = sin(uv.y * 40.0 + time * 6.0) * 0.004 * amount + (hash11(floor(uv.y * 120.0) + floor(time * 18.0)) - 0.5) * 0.006 * amount;
  w += smoothstep(0.15, 0.0, uv.y) * (hash11(floor(time * 18.0)) - 0.5) * 0.08 * amount;
  vec2 p = vec2(uv.x + w, uv.y);
  float bleed = 0.004 * amount;
  vec3 c = vec3(getColor(fold(p + vec2(bleed, 0.0))).r, getColor(fold(p)).g, getColor(fold(p - vec2(bleed, 0.0))).b);
  c = mix(c, vec3(luma(c)), 0.12 * amount);
  c += (hash21(floor(uv * vec2(ratio, 1.0) * 260.0) + floor(time * 24.0) * 11.0) - 0.5) * 0.08 * amount;
  return vec4(c, 1.0);
}
