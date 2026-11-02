//! MiniMax H3 drafts on disk. They live in `h3-pipeline`, which builds the same graphs for a scene rendered in the draft or
//! finalize phase. The Builder's routes and tests use them from here.

pub use h3_pipeline::drafts::{build_draft, build_finalize};
