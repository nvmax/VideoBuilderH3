// Author: H3 Studio
// License: H3 Studio
// Description: Printed dots, like a comic or newspaper
// Category: Retro
// Tags: effect, retro, halftone, dots, print, comic

// @param How big a dot cell is
uniform float size; // = 0.014 (0.004 .. 0.06)
// @param 0 is black ink, 1 keeps the picture's colour
uniform float colored; // = 1.0 (0.0 .. 1.0)

vec4 effect(vec2 uv) {
  vec2 p = rot2(centred(uv), 0.7854) / size;
  vec2 cell = floor(p) + 0.5;
  vec2 f = fract(p) - 0.5;
  vec3 c = getColor(fold(uncentred(rot2(cell * size, -0.7854)))).rgb;
  float r = sqrt(1.0 - luma(c)) * 0.72;
  float dotMask = smoothstep(r, r - 0.12, length(f));
  return vec4(mix(vec3(1.0), mix(vec3(0.0), c, colored), dotMask), 1.0);
}
