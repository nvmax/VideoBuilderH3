// Author: H3 Studio
// License: H3 Studio
// Description: The picture whips left out of the frame, smeared by its speed, and the next one whips in behind it
// Tags: slide, push, whip, motion-blur, trending, original

// @param How much the motion smears the pictures
uniform float blur; // = 1.0 (0.0 .. 2.0)
// @param Empty space left between the two pictures
uniform float gap; // = 0.0 (0.0 .. 0.4)

vec4 transition(vec2 uv) {
  return whip(uv, vec2(1.0, 0.0), blur, gap);
}
