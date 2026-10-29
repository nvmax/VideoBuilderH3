// Author: H3 Studio
// License: H3 Studio
// Description: The brightness flickers like a faulty lamp
// Category: Light
// Tags: effect, light, flicker, light, lamp

// @param How much the brightness changes
uniform float amount; // = 0.3 (0.0 .. 1.0)
// @param How fast it flickers
uniform float speed; // = 18.0 (2.0 .. 40.0)

vec4 effect(vec2 uv) {
  float f = 1.0 - amount * hash11(floor(time * speed));
  return vec4(getColor(uv).rgb * f, 1.0);
}
