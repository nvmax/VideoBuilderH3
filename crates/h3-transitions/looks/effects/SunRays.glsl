// Author: H3 Studio
// License: H3 Studio
// Description: Rays of light stream from a bright point
// Category: Light
// Tags: effect, light, rays, god rays, sun

// @param Where the light comes from
uniform vec2 center; // = vec2(0.8, 0.7)
// @param How bright a part must be to shine
uniform float threshold; // = 0.65 (0.0 .. 1.0)
// @param How strong the rays are
uniform float amount; // = 1.0 (0.0 .. 2.0)
// @param How long the rays are
uniform float length_; // = 0.6 (0.1 .. 1.0)

vec4 effect(vec2 uv) {
  vec3 c = getColor(uv).rgb;
  vec3 r = vec3(0.0);
  for (int i = 0; i < 20; i++) {
    float z = 1.0 - length_ * float(i) / 19.0;
    r += max(getColor(fold(center + (uv - center) * z)).rgb - vec3(threshold), vec3(0.0));
  }
  return vec4(c + r / 20.0 * amount * 3.0 * vec3(1.0, 0.92, 0.75), 1.0);
}
