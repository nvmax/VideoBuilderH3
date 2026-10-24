//! MiniMax H3 prompt text the Video Builder writes around the LLM's shot descriptions. Port of
//! `minimax/shot_prompt.py`.

pub(crate) mod rx;

pub mod assembly;
pub mod continuation;
pub mod cues;
pub mod emotion;
pub mod enhance;
pub mod frame;
pub mod lyric_free;
pub mod lyrics;
pub mod masked;
pub mod performance;
pub mod plan;
pub mod task;
pub mod text;
