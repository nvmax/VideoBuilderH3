//! Continuity modes. Port of the mode helpers in `minimax/scene_inputs.py`.

use serde_json::Value;

use crate::pyval::text_or_empty;

pub const MASKED: &str = "latent_continuation_masked";

/// `"_".join(str(value or "").strip().lower().replace("-", " ").split())`.
fn clean_key(value: Option<&Value>) -> String {
    text_or_empty(value).trim().to_lowercase().replace('-', " ").split_whitespace().collect::<Vec<_>>().join("_")
}

fn alias(clean: &str) -> Option<&'static str> {
    match clean {
        // The standard and exact-last-frame latent modes were retired. Old values continue masked.
        "latent_exact" | "latent_exact_frame" | "latent_continuation_exact" | "latent_continuation_exact_frame"
        | "latent_masked" | "latent_masked_av" | "latent_continuation_masked" | "latent" | "latent_continuation"
        | "continuation" => Some(MASKED),
        // The previous-final-frame modes were retired too. Old values mean off.
        "spatial" | "spatial_reference" | "continuity_reference" | "exact" | "exact_start" | "exact_start_frame"
        | "continuous_start" | "off" => Some("off"),
        _ => None,
    }
}

/// The canonical mode for any accepted spelling, or `None` when the text is not a continuity mode.
pub fn canonical_continuity_mode(value: Option<&Value>) -> Option<&'static str> {
    alias(&clean_key(value))
}

/// `latent_continuation_masked` for any spelling that means it, else `off`.
pub fn normalize_continuity_mode(value: Option<&Value>) -> &'static str {
    if alias(&clean_key(value)) == Some(MASKED) {
        MASKED
    } else {
        "off"
    }
}

/// Masked is unavailable in I2V, Image + Reference and 2 Pass Advanced (`three_pass`, which only applies to
/// Reference to Video).
pub fn continuity_allowed_for_mode(continuity: &str, mode: &str, render_pass: &str) -> bool {
    if continuity == MASKED {
        return mode != "image_to_video"
            && mode != "image_reference_to_video"
            && !(mode == "reference_to_video" && render_pass == "three_pass");
    }
    continuity == "off"
}
