// Author: H3 Studio
// License: H3 Studio
// Description: A hard zoom that snaps in and eases back
// Category: Camera
// Tags: effect, camera, punch, snap, zoom

// @param How hard it punches
uniform float zoom; // = 0.3 (0.0 .. 1.0)
// @param How fast it settles
uniform float decay; // = 5.0 (1.0 .. 20.0)

vec4 effect(vec2 uv) {
  float z = 1.0 + zoom * exp(-progress * decay);
  return getColor(fold(vec2(0.5) + (uv - 0.5) / z));
}
