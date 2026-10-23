//! ffmpeg and ffprobe wrapper, audio decode, peaks and beats.

pub mod audio;
pub mod tools;

pub use tools::{MediaError, Tools};
