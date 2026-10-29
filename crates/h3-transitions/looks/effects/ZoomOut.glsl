// Author: H3 Studio
// License: H3 Studio
// Description: A slow pull back over the block
// Category: Camera
// Tags: effect, camera, zoom, out, pull

// @param How far it starts zoomed in
uniform float zoom; // = 0.25 (0.0 .. 1.0)

vec4 effect(vec2 uv) {
  float z = 1.0 + zoom * (1.0 - ease(progress));
  return getColor(fold(vec2(0.5) + (uv - 0.5) / z));
}
