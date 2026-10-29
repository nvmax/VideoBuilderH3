// Author: H3 Studio
// License: H3 Studio
// Description: Scan lines tear and static crackles while the picture flickers over to the next one
// Tags: glitch, noise, static, digital, trending, original

// @param How violent the tearing is
uniform float intensity; // = 1.0 (0.0 .. 2.0)

vec4 transition(vec2 uv) {
  float t = progress;
  float amt = sin(PI * t) * intensity;
  float line = floor(uv.y * 120.0);
  float tick = floor(t * 24.0);
  float tear = step(0.8, hash21(vec2(line, tick))) * amt;
  vec2 p = vec2(uv.x + (hash21(vec2(line + 9.0, tick)) - 0.5) * 0.4 * tear, uv.y);
  float flicker = (hash11(tick) - 0.5) * 0.16;
  vec4 c = mix(getFromColor(fold(p)), getToColor(fold(p)), step(0.5 + flicker, t));
  float noise = hash21(floor(uv * vec2(ratio * 160.0, 90.0)) + tick * 13.0) - 0.5;
  c.rgb += noise * 0.3 * amt;
  c.rgb *= 1.0 - 0.18 * amt * step(0.5, fract(uv.y * 160.0));
  return vec4(c.rgb, 1.0);
}
