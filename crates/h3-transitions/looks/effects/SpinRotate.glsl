// Author: H3 Studio
// License: H3 Studio
// Description: The picture turns around its middle
// Category: Camera
// Tags: effect, camera, spin, rotate, turn

// @param How many full turns
uniform float turns; // = 1.0 (-4.0 .. 4.0)
// @param How much it zooms to hide the corners
uniform float zoom; // = 0.4 (0.0 .. 1.0)

vec4 effect(vec2 uv) {
  float a = turns * 2.0 * PI * ease(progress);
  vec2 p = rot2(centred(uv), a) / (1.0 + zoom * sin(PI * progress));
  return getColor(fold(uncentred(p)));
}
