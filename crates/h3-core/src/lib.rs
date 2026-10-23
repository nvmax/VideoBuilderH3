//! Pure MiniMax H3 logic: resolution, timing, token math, settings, scene inputs and shot prompts. No IO.

pub mod continuity;
pub mod error;
pub mod prompt;
pub mod pyjson;
pub mod pyval;
pub mod resolution;
pub mod scene;
pub mod settings;
pub mod tiles;
pub mod timing;
pub mod tokens;

pub use error::CoreError;
