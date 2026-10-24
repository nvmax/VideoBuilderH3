//! The Story Builder's steps with the selected LLM runner: Story Mode (book, acts, scene plan, places) and the story layer around scenes.

pub mod beat;
pub mod book;
pub mod context;
pub mod places;
pub mod plan;
pub mod revise;
#[cfg(test)]
mod golden;
