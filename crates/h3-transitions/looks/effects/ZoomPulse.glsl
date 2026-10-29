// Author: H3 Studio
// License: H3 Studio
// Description: A quick push in and back out
// Category: Camera
// Tags: effect, camera, zoom, pulse, beat

// @param How far it pushes in
uniform float zoom; // = 0.15 (0.0 .. 0.6)

vec4 effect(vec2 uv) {
  float z = 1.0 + zoom * sin(PI * progress);
  return getColor(fold(vec2(0.5) + (uv - 0.5) / z));
}
