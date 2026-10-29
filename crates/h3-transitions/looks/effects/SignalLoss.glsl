// Author: H3 Studio
// License: H3 Studio
// Description: Torn bands and bursts of static, like a failing signal
// Category: Glitch
// Tags: effect, glitch, signal, static, tear

// @param How broken the signal is
uniform float amount; // = 0.6 (0.0 .. 1.0)
// @param How many bands tear
uniform float bands; // = 6.0 (1.0 .. 20.0)

vec4 effect(vec2 uv) {
  float band = floor(uv.y * bands * 4.0 + time * 6.0);
  float h = hash11(band);
  float tear = step(0.7, h) * (hash11(band + 13.0) - 0.5) * amount * 0.4;
  vec3 c = getColor(fold(vec2(uv.x + tear, uv.y))).rgb;
  float noise = hash21(floor(uv * vec2(ratio, 1.0) * 220.0) + floor(time * 30.0) * 17.0);
  c = mix(c, vec3(noise), step(0.92, h) * amount * 0.8);
  c += (hash21(vec2(floor(uv.y * 400.0), floor(time * 30.0))) - 0.5) * 0.08 * amount;
  return vec4(c, 1.0);
}
