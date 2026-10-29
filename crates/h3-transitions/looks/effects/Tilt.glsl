// Author: H3 Studio
// License: H3 Studio
// Description: A fixed dutch angle
// Category: Camera
// Tags: effect, camera, tilt, dutch, angle

// @param The angle, in degrees
uniform float angle; // = 8.0 (-45.0 .. 45.0)
// @param How much it zooms to hide the corners
uniform float zoom; // = 0.2 (0.0 .. 0.8)

vec4 effect(vec2 uv) {
  vec2 p = rot2(centred(uv), radians(angle)) / (1.0 + zoom);
  return getColor(fold(uncentred(p)));
}
