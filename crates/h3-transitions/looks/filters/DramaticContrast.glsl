// Author: H3 Studio
// License: H3 Studio
// Description: Strong contrast, deep shadows and a vignette
// Category: Cinematic
// Tags: filter, cinematic, contrast, drama

// @param Brightness, in stops
uniform float exposure; // = 0.000 (-1.0 .. 1.0)
// @param How far the tones are pulled apart around the middle grey
uniform float contrast; // = 0.450 (-1.0 .. 1.0)
// @param How strong the colours are (-1 is black and white)
uniform float saturation; // = 0.100 (-1.0 .. 1.0)
// @param Cool (blue) to warm (orange)
uniform float temperature; // = 0.000 (-1.0 .. 1.0)
// @param Green to magenta
uniform float tint; // = 0.000 (-1.0 .. 1.0)
// @param Brighter or darker bright areas
uniform float highlights; // = -0.150 (-1.0 .. 1.0)
// @param Lighter or deeper dark areas
uniform float shadows; // = -0.150 (-1.0 .. 1.0)
// @param Lifts the blacks for a matte, faded print
uniform float fade; // = 0.000 (0.0 .. 1.0)
// @param Brightens or darkens the middle tones
uniform float gamma; // = 1.000 (0.5 .. 2.0)
// @param The colour the dark areas lean towards
uniform vec3 shadowTone; // = vec3(0.500, 0.500, 0.500) color
// @param The colour the bright areas lean towards
uniform vec3 highlightTone; // = vec3(0.500, 0.500, 0.500) color
// @param How much of the split tone is used
uniform float toneAmount; // = 0.000 (0.0 .. 1.0)
// @param How much of the picture turns black and white
uniform float mono; // = 0.000 (0.0 .. 1.0)
// @param How much red, green and blue make the grey (a red filter is high red)
uniform vec3 monoWeights; // = vec3(0.299, 0.587, 0.114)
// @param Darkens the corners
uniform float vignette; // = 0.300 (0.0 .. 1.0)
// @param Film grain
uniform float grain; // = 0.000 (0.0 .. 1.0)
// @param Crisper detail
uniform float sharpen; // = 0.000 (0.0 .. 1.0)

vec4 effect(vec2 uv) {
  vec3 c = getColor(uv).rgb;
  c = sharpenAt(uv, c, sharpen);
  c = applyLook(c, uv, exposure, contrast, saturation, temperature, tint, highlights, shadows, fade, gamma, shadowTone, highlightTone, toneAmount, mono, monoWeights, vignette, grain);
  return vec4(c, 1.0);
}
