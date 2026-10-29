// Author: H3 Studio
// License: H3 Studio
// Description: A slow slide across the picture
// Category: Camera
// Tags: effect, camera, pan, slide, drift

// @param How far it slides sideways
uniform float dx; // = 0.08 (-0.5 .. 0.5)
// @param How far it slides up
uniform float dy; // = 0.0 (-0.5 .. 0.5)

vec4 effect(vec2 uv) {
  return getColor(fold(uv + vec2(dx, dy) * (progress - 0.5)));
}
