// Author: H3 Studio
// License: H3 Studio
// Description: Warm light leaks sweep across the frame and burn out the cut
// Tags: light, leak, glow, film, trending, original

// @param How strong the light is
uniform float intensity; // = 1.0 (0.0 .. 2.0)

vec4 transition(vec2 uv) {
  float t = progress;
  vec4 base = mix(getFromColor(uv), getToColor(uv), smoothstep(0.35, 0.65, t));
  float amt = pow(sin(PI * t), 1.4) * intensity;
  vec2 d1 = (uv - vec2(mix(-0.3, 1.3, t), 0.25 + 0.2 * sin(t * 5.0))) * vec2(0.7, 1.6);
  vec2 d2 = (uv - vec2(mix(1.3, -0.3, t), 0.8 - 0.15 * sin(t * 4.0))) * vec2(0.5, 1.2);
  float blobs = exp(-dot(d1, d1) * 5.0) + 0.7 * exp(-dot(d2, d2) * 4.0);
  float glow = clamp(blobs * (0.7 + 0.6 * vnoise(uv * 3.0 + t * 2.0)), 0.0, 1.5);
  vec3 leak = mix(vec3(1.0, 0.25, 0.05), vec3(1.0, 0.85, 0.45), clamp(glow, 0.0, 1.0));
  vec3 lit = 1.0 - (1.0 - base.rgb) * (1.0 - leak * glow * amt);
  return vec4(lit, 1.0);
}
