// Author: H3 Studio
// License: H3 Studio
// Description: A slow push in over the block
// Category: Camera
// Tags: effect, camera, zoom, in, push

// @param How far it pushes in
uniform float zoom; // = 0.25 (0.0 .. 1.0)

vec4 effect(vec2 uv) {
  float z = 1.0 + zoom * ease(progress);
  return getColor(fold(vec2(0.5) + (uv - 0.5) / z));
}
