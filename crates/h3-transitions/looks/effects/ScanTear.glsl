// Author: H3 Studio
// License: H3 Studio
// Description: A bright tear line sweeps down the picture and drags it sideways
// Category: Glitch
// Tags: effect, glitch, scan, tear, sweep

// @param How tall the tear is
uniform float width; // = 0.12 (0.02 .. 0.4)
// @param How far it drags the picture
uniform float amount; // = 0.1 (0.0 .. 0.4)
// @param How fast it sweeps
uniform float speed; // = 0.6 (0.1 .. 3.0)

vec4 effect(vec2 uv) {
  float y = 1.0 - fract(time * speed);
  float band = smoothstep(width, 0.0, abs(uv.y - y));
  float jitter = hash11(floor(uv.y * 80.0) + floor(time * 24.0));
  vec3 c = getColor(fold(vec2(uv.x + band * amount * (jitter - 0.3), uv.y))).rgb;
  return vec4(c + band * 0.12, 1.0);
}
