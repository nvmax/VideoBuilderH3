//! Pure MiniMax H3 logic: resolution, timing, token math, settings, scene inputs and shot prompts. No IO.

pub mod continuity;
pub mod error;
pub mod prompt;
pub mod pyjson;
pub mod pyval;
pub mod refmod;
pub mod resolution;
pub mod scene;
pub mod scene_card;
pub mod settings;
pub mod style;
pub mod tiles;
pub mod lyric_scenes;
pub mod timed_lines;
pub mod timeline;
pub mod timeline_markers;
pub mod timing;
pub mod tokens;

pub use error::CoreError;
