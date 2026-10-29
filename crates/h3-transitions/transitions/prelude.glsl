// Helpers every transition of the app can use. They are put in front of each transition's source, in the preview and in the final render, and
// they only call getFromColor, getToColor, progress and ratio, which both have.

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

// A position folded back into the picture, so a zoomed or shifted sample repeats the picture's edge instead of smearing it.
vec2 fold(vec2 p) { return 1.0 - abs(1.0 - mod(p, vec2(2.0))); }

// 1 when the position is inside the picture, else 0.
float inside(vec2 p) { return step(0.0, p.x) * step(p.x, 1.0) * step(0.0, p.y) * step(p.y, 1.0); }

// A zoom of the picture about its centre. Taking samples at slightly different zooms smears it radially.
vec4 zoomFrom(vec2 uv, float zoom, float blur) {
  vec4 sum = vec4(0.0);
  for (int i = 0; i < 10; i++) {
    float z = zoom * (1.0 - blur * (float(i) / 9.0) * 0.3);
    sum += getFromColor(fold((uv - 0.5) / z + 0.5));
  }
  return sum / 10.0;
}
vec4 zoomTo(vec2 uv, float zoom, float blur) {
  vec4 sum = vec4(0.0);
  for (int i = 0; i < 10; i++) {
    float z = zoom * (1.0 - blur * (float(i) / 9.0) * 0.3);
    sum += getToColor(fold((uv - 0.5) / z + 0.5));
  }
  return sum / 10.0;
}

// Both pictures move along `dir` (the picture before leaves, the next one arrives), smeared along it while they move fast, with `gap` of empty
// space between them.
vec4 whip(vec2 uv, vec2 dir, float blur, float gap) {
  vec2 d = normalize(dir);
  float travel = 1.0 + gap;
  float e = ease(progress);
  float length_ = blur * 0.22 * sin(PI * progress);
  vec4 sum = vec4(0.0);
  float weight = 0.0;
  for (int i = 0; i < 12; i++) {
    vec2 o = d * length_ * (float(i) / 11.0 - 0.5);
    vec2 pa = uv + d * e * travel + o;
    vec2 pb = uv - d * (1.0 - e) * travel + o;
    float ia = inside(pa);
    float ib = inside(pb);
    sum += ia * getFromColor(pa) + ib * getToColor(pb);
    weight += ia + ib;
  }
  return weight > 0.0 ? sum / weight : vec4(0.0, 0.0, 0.0, 1.0);
}
