// Author: H3 Studio
// License: H3 Studio
// Description: Flicker, grain, scratches and dust like an old print
// Category: Retro
// Tags: effect, retro, old film, scratches, dust, flicker

// @param How worn the film is
uniform float amount; // = 0.7 (0.0 .. 1.0)
// @param How much the brightness flickers
uniform float flicker; // = 0.12 (0.0 .. 0.4)

vec4 effect(vec2 uv) {
  float t = floor(time * 18.0);
  vec3 c = getColor(uv).rgb;
  c *= 1.0 - flicker * hash11(t);
  c += (hash21(floor(uv * vec2(ratio, 1.0) * 500.0) + t * 13.0) - 0.5) * 0.16 * amount;
  float sx = hash11(t + 3.0);
  c += step(abs(uv.x - sx), 0.0012) * step(0.6, hash11(t + 9.0)) * 0.5 * amount;
  vec2 dp = vec2(hash11(t + 21.0), hash11(t + 29.0));
  c = mix(c, vec3(0.05), smoothstep(0.012, 0.004, length((uv - dp) * vec2(ratio, 1.0))) * step(0.5, hash11(t + 33.0)) * amount);
  c *= 1.0 - 0.45 * amount * smoothstep(0.35, 1.1, length(centred(uv)));
  return vec4(c, 1.0);
}
