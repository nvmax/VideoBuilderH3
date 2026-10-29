// Author: H3 Studio
// License: H3 Studio
// Description: Bands of the picture tear sideways and split into red, green and blue, each cutting over to the next picture at its own moment
// Tags: glitch, rgb, digital, trending, original

// @param How violent the glitch is
uniform float intensity; // = 1.0 (0.0 .. 2.0)

vec4 split3(float from_, vec2 p, float split) {
  vec2 o = vec2(split, 0.0);
  vec4 r = from_ > 0.5 ? getFromColor(fold(p + o)) : getToColor(fold(p + o));
  vec4 g = from_ > 0.5 ? getFromColor(fold(p)) : getToColor(fold(p));
  vec4 b = from_ > 0.5 ? getFromColor(fold(p - o)) : getToColor(fold(p - o));
  return vec4(r.r, g.g, b.b, 1.0);
}

vec4 transition(vec2 uv) {
  float t = progress;
  float amt = sin(PI * t) * intensity;
  float band = floor(uv.y * 28.0);
  float tick = floor(t * 16.0);
  float torn = step(0.62, hash21(vec2(band, tick))) * amt;
  float shift = (hash21(vec2(band + 40.0, tick)) - 0.5) * 0.25 * torn;
  float split = 0.012 * amt + 0.03 * torn;
  vec2 p = vec2(uv.x + shift, uv.y);
  float cut = step(0.2 + 0.6 * hash11(band), t);
  return mix(split3(1.0, p, split), split3(0.0, p, split), cut);
}
