// Author: H3 Studio
// License: H3 Studio
// Description: Glowing coloured outlines on black
// Category: Stylize
// Tags: effect, stylize, neon, edges, glow

// @param How strong the lines are
uniform float gain; // = 3.0 (0.5 .. 10.0)
// @param How far the colour changes across the picture
uniform float shift; // = 0.4 (0.0 .. 1.0)

vec4 effect(vec2 uv) {
  float e = clamp(edgeAt(uv, 0.0022) * gain, 0.0, 1.0);
  vec3 col = hsv2rgb(vec3(fract(0.55 + uv.x * shift + time * 0.1), 0.85, 1.0));
  return vec4(col * e * 1.4, 1.0);
}
