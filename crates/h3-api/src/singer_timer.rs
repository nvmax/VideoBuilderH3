//! Timing a scene's sung words with Stable-ts in ComfyUI, for the MiniMax prompt job. The Rust side of `time_scene_singer_cues` in
//! `agent_api/orchestrator/lyrics_orchestrator.py`: it cuts the scene's own audio, runs the timestamp workflow on it and reads the
//! timed words back. What to time, and how the words fall into the shots, is decided in `h3_core::prompt::singer_timing`.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use h3_core::prompt::singer_timing::TimingPlan;
use h3_core::pyval::{text_or_empty, Obj};
use h3_core::timed_lines::parse_timestamped_lyrics_output;
use h3_jobs::JobCtx;
use h3_pipeline::minimax_prompts::{SingerTimer, TimerError, TimerFuture};
use serde_json::Value;

use crate::app::AppState;
use crate::job_handlers::context_for;

/// The nodes the timestamp workflow needs.
const NODES: [&str; 3] = ["VRGDG_TimestampedLyricsExtractor", "VRGDG_GetStems", "VRGDG_ShowText"];

/// Times scenes for one job: the cancel switch and the prompt id are the job's.
pub struct ComfyTimer<'a> {
    pub state: &'a Arc<AppState>,
    pub ctx: &'a JobCtx,
    pub folder: PathBuf,
}

impl SingerTimer for ComfyTimer<'_> {
    fn project_folder(&self) -> String {
        self.folder.display().to_string()
    }

    fn time<'a>(&'a self, plan: &'a TimingPlan) -> TimerFuture<'a> {
        Box::pin(async move {
            // The scene's own audio: this scene's part of the song, or its custom clip.
            let env = self.state.builder_env();
            let request = plan.clip.clone();
            let clip = tokio::task::spawn_blocking(move || h3_builder::runner::prepare_scene_audio_clip(&env, &request))
                .await
                .map_err(|e| TimerError::Comfy(format!("The request failed: {e}")))?
                .map_err(TimerError::Scene)?;
            let audio = text_or_empty(clip.get("audio_path"));
            let graph = h3_graph::timestamp::timestamp_graph(&plan.graph_request(&audio)).map_err(|e| TimerError::Scene(e.to_string()))?;

            let (run, _) = context_for(self.state, self.ctx).await.map_err(|e| TimerError::Comfy(e.message))?;
            if let Some(known) = run.env.object_info.as_object().filter(|m| !m.is_empty()) {
                let missing: Vec<&str> = NODES.into_iter().filter(|class| !known.contains_key(*class)).collect();
                if !missing.is_empty() {
                    return Err(TimerError::Comfy(format!("ComfyUI is missing these nodes: {}. Install or update the VRGDG node pack.", missing.join(", "))));
                }
            }
            let graph = Value::Object(graph);
            // The progress socket is opened first, under the id the prompt is queued with, so no step is missed.
            let client_id = h3_comfy::unique_client_id();
            let feed = run.client.progress_feed(&client_id).await;
            let queued = run.client.queue_prompt_as(&graph, &client_id).await.map_err(|e| TimerError::Comfy(e.to_string()))?;
            self.ctx.set_prompt_id(&queued.prompt_id);
            let history = run
                .client
                .wait_for_prompt_live(&queued.prompt_id, &graph, feed.as_ref(), Duration::from_secs(45 * 60), Duration::from_millis(500), &self.ctx.cancel, |_, _| {})
                .await
                .map_err(|e| TimerError::Comfy(e.to_string()))?;
            drop(feed);
            let texts = h3_comfy::extract_texts(&history, &queued.prompt_id);
            let payload: Obj = parse_timestamped_lyrics_output(&texts.join("\n")).map_err(TimerError::Scene)?;
            Ok(payload)
        })
    }
}
