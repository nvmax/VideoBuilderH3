// Author: H3 Studio
// License: H3 Studio
// Description: Big square pixels
// Category: Retro
// Tags: effect, retro, pixelate, pixels, mosaic

// @param How big a pixel is (the picture's height is 1)
uniform float size; // = 0.03 (0.005 .. 0.2)

vec4 effect(vec2 uv) {
  vec2 cell = (floor(uv * vec2(ratio, 1.0) / size) + 0.5) * size / vec2(ratio, 1.0);
  return getColor(cell);
}
