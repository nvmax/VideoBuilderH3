// Author: H3 Studio
// License: H3 Studio
// Description: The picture bounces on every beat of the block
// Category: Camera
// Tags: effect, camera, beat, bounce, pulse

// @param How many bounces in the block
uniform int beats; // = 4 (1 .. 16)
// @param How big a bounce is
uniform float zoom; // = 0.1 (0.0 .. 0.5)

vec4 effect(vec2 uv) {
  float z = 1.0 + zoom * exp(-fract(progress * float(beats)) * 6.0);
  return getColor(fold(vec2(0.5) + (uv - 0.5) / z));
}
