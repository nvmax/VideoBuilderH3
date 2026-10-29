// Helpers every effect and filter of the app can use. They are put in front of each look's source, in the preview and in the final render, and
// they only call getColor and read progress, ratio and time, which both have. Sizes are fractions of the picture's height, so a look is the same
// at any resolution.

const float PI = 3.14159265;

float ease(float t) { return t * t * (3.0 - 2.0 * t); }

// An integer hash, so the preview and the final render make the same noise on every graphics card.
uint pcg(uint v) {
  uint state = v * 747796405u + 2891336453u;
  uint word = ((state >> ((state >> 28u) + 4u)) ^ state) * 277803737u;
  return (word >> 22u) ^ word;
}
float hash11(float p) { return float(pcg(uint(max(p, 0.0) * 64.0 + 1.0))) / 4294967295.0; }
float hash21(vec2 p) { return float(pcg(pcg(uint(max(p.x, 0.0))) ^ uint(max(p.y, 0.0)))) / 4294967295.0; }

float vnoise(vec2 p) {
  vec2 i = floor(p);
  vec2 f = fract(p);
  f = f * f * (3.0 - 2.0 * f);
  float a = hash21(i);
  float b = hash21(i + vec2(1.0, 0.0));
  float c = hash21(i + vec2(0.0, 1.0));
  float d = hash21(i + vec2(1.0, 1.0));
  return mix(mix(a, b, f.x), mix(c, d, f.x), f.y);
}

vec2 rot2(vec2 v, float a) {
  float c = cos(a);
  float s = sin(a);
  return vec2(c * v.x - s * v.y, s * v.x + c * v.y);
}

// A position folded back into the picture, so a shifted or zoomed sample repeats the picture's edge instead of smearing it.
vec2 fold(vec2 p) { return 1.0 - abs(1.0 - mod(p, vec2(2.0))); }

// 1 when the position is inside the picture, else 0.
float inside(vec2 p) { return step(0.0, p.x) * step(p.x, 1.0) * step(0.0, p.y) * step(p.y, 1.0); }

// A position in units of the picture's height, centred, so a circle is round.
vec2 centred(vec2 uv) { return (uv - 0.5) * vec2(ratio, 1.0); }
vec2 uncentred(vec2 p) { return p / vec2(ratio, 1.0) + 0.5; }

float luma(vec3 c) { return dot(c, vec3(0.2126, 0.7152, 0.0722)); }

vec3 rgb2hsv(vec3 c) {
  vec4 k = vec4(0.0, -1.0 / 3.0, 2.0 / 3.0, -1.0);
  vec4 p = mix(vec4(c.bg, k.wz), vec4(c.gb, k.xy), step(c.b, c.g));
  vec4 q = mix(vec4(p.xyw, c.r), vec4(c.r, p.yzx), step(p.x, c.r));
  float d = q.x - min(q.w, q.y);
  float e = 1.0e-10;
  return vec3(abs(q.z + (q.w - q.y) / (6.0 * d + e)), d / (q.x + e), q.x);
}

vec3 hsv2rgb(vec3 c) {
  vec3 p = abs(fract(c.xxx + vec3(1.0, 2.0 / 3.0, 1.0 / 3.0)) * 6.0 - 3.0);
  return c.z * mix(vec3(1.0), clamp(p - 1.0, 0.0, 1.0), c.y);
}

// Nine taps along `step` (a distance in uv), weighted like a gaussian.
vec4 blurLine(vec2 uv, vec2 step) {
  vec4 sum = getColor(uv) * 0.2270270;
  sum += (getColor(uv + step * 1.0) + getColor(uv - step * 1.0)) * 0.1945946;
  sum += (getColor(uv + step * 2.0) + getColor(uv - step * 2.0)) * 0.1216216;
  sum += (getColor(uv + step * 3.0) + getColor(uv - step * 3.0)) * 0.0540541;
  sum += (getColor(uv + step * 4.0) + getColor(uv - step * 4.0)) * 0.0162162;
  return sum;
}

// A soft blur of the given radius (a fraction of the height): two passes of nine taps cannot be chained here, so this takes a ring of samples.
vec4 blurDisc(vec2 uv, float radius) {
  vec4 sum = getColor(uv);
  float weight = 1.0;
  for (int i = 0; i < 16; i++) {
    float a = float(i) * 2.399963;
    float r = sqrt((float(i) + 0.5) / 16.0) * radius;
    vec2 o = vec2(cos(a), sin(a)) * r / vec2(ratio, 1.0);
    sum += getColor(fold(uv + o));
    weight += 1.0;
  }
  return sum / weight;
}

// A zoom blur about `centre`: samples at slightly different zooms smear the picture radially. `amount` is how far the outer samples reach.
vec4 radialBlur(vec2 uv, vec2 centre, float amount) {
  vec4 sum = vec4(0.0);
  for (int i = 0; i < 14; i++) {
    float z = 1.0 - amount * (float(i) / 13.0);
    sum += getColor(fold((uv - centre) * z + centre));
  }
  return sum / 14.0;
}

// A blur along `dir` (a vector of length `length_` in height units), centred on the pixel.
vec4 motionBlur(vec2 uv, vec2 dir, float length_) {
  vec2 d = normalize(dir) * length_ / vec2(ratio, 1.0);
  vec4 sum = vec4(0.0);
  for (int i = 0; i < 14; i++) {
    float t = float(i) / 13.0 - 0.5;
    sum += getColor(fold(uv + d * t));
  }
  return sum / 14.0;
}

// The brightness of the picture at a position, for edge finding.
float lumaAt(vec2 uv) { return luma(getColor(fold(uv)).rgb); }

// The strength of an edge at `uv` (Sobel), with taps `px` apart (a fraction of the height).
float edgeAt(vec2 uv, float px) {
  vec2 o = vec2(px / ratio, px);
  float tl = lumaAt(uv + vec2(-o.x, o.y));
  float t = lumaAt(uv + vec2(0.0, o.y));
  float tr = lumaAt(uv + vec2(o.x, o.y));
  float l = lumaAt(uv + vec2(-o.x, 0.0));
  float r = lumaAt(uv + vec2(o.x, 0.0));
  float bl = lumaAt(uv + vec2(-o.x, -o.y));
  float b = lumaAt(uv + vec2(0.0, -o.y));
  float br = lumaAt(uv + vec2(o.x, -o.y));
  float gx = -tl - 2.0 * l - bl + tr + 2.0 * r + br;
  float gy = -tl - 2.0 * t - tr + bl + 2.0 * b + br;
  return sqrt(gx * gx + gy * gy);
}

// Sharpening: the picture plus its difference from a four tap blur.
vec3 sharpenAt(vec2 uv, vec3 c, float amount) {
  if (amount <= 0.0) return c;
  vec2 o = vec2(0.0035 / ratio, 0.0035);
  vec3 blur = (getColor(fold(uv + vec2(o.x, 0.0))).rgb + getColor(fold(uv - vec2(o.x, 0.0))).rgb + getColor(fold(uv + vec2(0.0, o.y))).rgb + getColor(fold(uv - vec2(0.0, o.y))).rgb) * 0.25;
  return c + (c - blur) * amount * 2.5;
}

// The colour grade every filter is made of. The numbers are the filter's settings: exposure and contrast, saturation, a warm or cool and a green or
// magenta shift, highlights and shadows, faded blacks, gamma, a split tone (a colour for the shadows and one for the highlights, `toneAmount` of it),
// a mono mix with its channel weights, a vignette and film grain. With every setting at its neutral value the colour comes out as it went in.
vec3 applyLook(vec3 c, vec2 uv, float exposure, float contrast, float saturation, float temperature, float tint, float highlights, float shadows, float fade, float gamma,
               vec3 shadowTone, vec3 highlightTone, float toneAmount, float mono, vec3 monoWeights, float vignette, float grain) {
  c *= exp2(exposure);
  c = pow(max(c, vec3(0.0)), vec3(1.0 / max(gamma, 0.05)));
  float l = luma(c);
  c += shadows * (1.0 - smoothstep(0.0, 0.55, l)) * 0.3;
  c += highlights * smoothstep(0.45, 1.0, l) * 0.3;
  c = (c - 0.5) * (1.0 + contrast) + 0.5;
  c *= vec3(1.0 + 0.16 * temperature + 0.04 * tint, 1.0 - 0.08 * tint, 1.0 - 0.16 * temperature + 0.04 * tint);
  c = max(c, vec3(0.0));
  float l2 = luma(c);
  c = mix(vec3(l2), c, 1.0 + saturation);
  c = mix(c, vec3(dot(c, monoWeights)), mono);
  if (toneAmount > 0.0) {
    float lt = luma(c);
    vec3 tone = mix(shadowTone, highlightTone, smoothstep(0.15, 0.85, lt));
    vec3 t = c * tone * 2.0;
    t *= (lt + 0.002) / (luma(t) + 0.002);
    c = mix(c, t, toneAmount);
  }
  c = c * (1.0 - 0.5 * fade) + 0.15 * fade;
  vec2 p = centred(uv);
  c *= 1.0 - vignette * smoothstep(0.3, 0.95, length(p));
  if (grain > 0.0) {
    float n = hash21(floor(uv * vec2(ratio, 1.0) * 540.0) + floor(time * 24.0) * 37.0);
    c += (n - 0.5) * grain * 0.3 * (0.4 + 0.6 * (1.0 - abs(luma(c) - 0.5) * 2.0));
  }
  return clamp(c, 0.0, 1.0);
}
