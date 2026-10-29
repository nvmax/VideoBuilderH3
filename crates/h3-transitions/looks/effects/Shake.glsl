// Author: H3 Studio
// License: H3 Studio
// Description: The camera shakes, with a little roll
// Category: Camera
// Tags: effect, camera, shake, camera, hit

// @param How far the picture jumps
uniform float amount; // = 0.02 (0.0 .. 0.1)
// @param How many jumps a second
uniform float speed; // = 24.0 (4.0 .. 60.0)
// @param How far it rolls, in degrees
uniform float roll; // = 0.6 (0.0 .. 4.0)

vec4 effect(vec2 uv) {
  float t = floor(time * speed);
  vec2 j = (vec2(hash11(t), hash11(t + 19.0)) - 0.5) * 2.0 * amount;
  float r = (hash11(t + 37.0) - 0.5) * 2.0 * radians(roll);
  return getColor(fold(uncentred(rot2(centred(uv), r)) + j));
}
