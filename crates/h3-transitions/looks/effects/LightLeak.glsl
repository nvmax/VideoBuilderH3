// Author: H3 Studio
// License: H3 Studio
// Description: A warm leak of light sweeps across the picture
// Category: Light
// Tags: effect, light, light, leak, film, warm

// @param How strong the leak is
uniform float amount; // = 0.8 (0.0 .. 1.5)

vec4 effect(vec2 uv) {
  vec3 c = getColor(uv).rgb;
  float x = mix(-0.3, 1.3, progress);
  float d = length((uv - vec2(x, 0.85)) * vec2(1.0, 1.6));
  float leak = exp(-d * d * 5.0);
  vec3 col = mix(vec3(1.0, 0.30, 0.08), vec3(1.0, 0.85, 0.40), leak);
  return vec4(vec3(1.0) - (vec3(1.0) - c) * (vec3(1.0) - col * leak * amount), 1.0);
}
