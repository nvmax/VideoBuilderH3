// Author: H3 Studio
// License: H3 Studio
// Description: A few flat colours with an ordered dot pattern, like an early computer
// Category: Retro
// Tags: effect, retro, dither, pixel, retro computer

// @param How big a dot is
uniform float size; // = 0.004 (0.002 .. 0.02)
// @param How many levels of each colour
uniform int levels; // = 3 (2 .. 8)

float bayer2(vec2 a) { a = floor(a); return fract(a.x / 2.0 + a.y * a.y * 0.75); }
float bayer4(vec2 a) { return bayer2(0.5 * a) * 0.25 + bayer2(a); }
vec4 effect(vec2 uv) {
  vec2 pixel = floor(uv * vec2(ratio, 1.0) / size);
  vec2 cell = (pixel + 0.5) * size / vec2(ratio, 1.0);
  vec3 c = getColor(cell).rgb;
  float n = float(levels);
  return vec4(floor(c * (n - 1.0) + bayer4(pixel)) / (n - 1.0), 1.0);
}
