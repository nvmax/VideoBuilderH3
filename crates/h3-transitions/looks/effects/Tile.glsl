// Author: H3 Studio
// License: H3 Studio
// Description: The picture repeated in a grid
// Category: Distort
// Tags: effect, distort, tile, repeat, grid

// @param How many tiles across and down
uniform int count; // = 2 (1 .. 8)

vec4 effect(vec2 uv) { return getColor(fract(uv * float(count))); }
