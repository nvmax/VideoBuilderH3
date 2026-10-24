//! The Story Builder's steps with the selected LLM runner: Story Mode (book, acts, scene plan, places) and the story layer around scenes.

pub mod arc;
pub mod beat;
pub mod book;
pub mod context;
pub mod dialogue_scenes;
pub mod places;
pub mod plan;
pub mod revise;
pub mod scene_prompts;
#[cfg(test)]
mod golden;
