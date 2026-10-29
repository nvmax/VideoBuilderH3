// Author: H3 Studio
// License: H3 Studio
// Description: A slow, smooth wander like a hand holding the camera
// Category: Camera
// Tags: effect, camera, handheld, drift, smooth

// @param How far the picture wanders
uniform float amount; // = 0.012 (0.0 .. 0.06)
// @param How fast
uniform float speed; // = 1.2 (0.2 .. 6.0)
// @param How far it rolls, in degrees
uniform float roll; // = 0.5 (0.0 .. 3.0)

vec4 effect(vec2 uv) {
  float t = time * speed;
  vec2 j = (vec2(vnoise(vec2(t, 1.3)), vnoise(vec2(t, 7.7))) - 0.5) * 2.0 * amount;
  float r = (vnoise(vec2(t, 3.1)) - 0.5) * 2.0 * radians(roll);
  return getColor(fold(uncentred(rot2(centred(uv), r)) + j));
}
