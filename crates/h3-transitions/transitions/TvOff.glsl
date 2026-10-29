// Author: H3 Studio
// License: H3 Studio
// Description: An old television switching off: the picture collapses to a glowing line, then the next one opens out of it
// Tags: tv, crt, retro, shape, trending, original

vec4 transition(vec2 uv) {
  float t = progress;
  float cy = abs(uv.y - 0.5);
  float line = 0.0035;
  float k = t < 0.5 ? ease(t * 2.0) : ease((1.0 - t) * 2.0);
  float h = 1.0 - k;
  float visible = step(cy, 0.5 * h + line * k);
  vec2 p = vec2(uv.x, 0.5 + (uv.y - 0.5) / max(h, 0.02));
  vec4 c = mix(getFromColor(fold(p)), getToColor(fold(p)), step(0.5, t)) * visible;
  c.rgb += vec3(0.9, 0.95, 1.0) * exp(-cy * 90.0) * k * 0.8;
  return vec4(c.rgb, 1.0);
}
