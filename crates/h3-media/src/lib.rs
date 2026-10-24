//! ffmpeg and ffprobe wrapper, audio decode, peaks and beats.

pub mod audio;
pub mod collect;
pub mod effect;
pub mod grade;
pub mod peaks;
pub mod post;
pub mod slideshow;
pub mod stitch;
pub mod tools;
pub mod video;

pub use tools::{MediaError, Tools};
