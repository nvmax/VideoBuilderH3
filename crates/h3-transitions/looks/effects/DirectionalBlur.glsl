// Author: H3 Studio
// License: H3 Studio
// Description: A blur along one direction
// Category: Blur
// Tags: effect, blur, blur, motion, directional

// @param How long the blur is
uniform float length_; // = 0.05 (0.0 .. 0.3)
// @param The direction, in degrees
uniform float angle; // = 0.0 (0.0 .. 360.0)

vec4 effect(vec2 uv) {
  float a = radians(angle);
  return motionBlur(uv, vec2(cos(a), sin(a)), length_);
}
