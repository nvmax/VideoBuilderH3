// Author: H3 Studio
// License: H3 Studio
// Description: The picture is pulled in with a zoom blur and the next one settles out of it
// Tags: zoom, blur, pull, trending, original

// @param How far the picture is pulled in
uniform float strength; // = 0.6 (0.0 .. 2.0)
// @param How much the zoom smears the picture
uniform float blur; // = 1.0 (0.0 .. 1.0)

vec4 transition(vec2 uv) {
  float e = ease(progress);
  vec4 a = zoomFrom(uv, 1.0 + strength * e, blur * min(progress * 2.0, 1.0));
  vec4 b = zoomTo(uv, 1.0 + strength * (1.0 - e), blur * min((1.0 - progress) * 2.0, 1.0));
  return mix(a, b, smoothstep(0.35, 0.65, progress));
}
