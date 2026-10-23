//! MiniMax H3 prompt text the Video Builder writes around the LLM's shot descriptions. Port of
//! `minimax/shot_prompt.py`.

pub(crate) mod rx;

pub mod continuation;
pub mod cues;
pub mod emotion;
pub mod frame;
pub mod lyrics;
pub mod performance;
pub mod plan;
pub mod task;
pub mod text;
