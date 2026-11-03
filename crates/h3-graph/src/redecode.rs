//! Make a MiniMax H3 scene video again from its saved latent, at a chosen quality. Port of `runner/minimax_redecode.py`.
//!
//! A render saves the sampler's whole output (picture and sound) to `latents/scene_NNN.latent` right before the VAE decode.
//! Decoding it again gives the same frames the render encoded, and the encoder can then be given a finer CRF than the render
//! used. The graph the scene compiles to is cut down to what that needs: load the latent, decode, resize, encode. Nothing is
//! sampled again.

use std::collections::HashSet;

use h3_core::pyval::Obj;
use h3_core::tokens::tokens_to_frames;
use serde_json::{json, Value};

use crate::api::{as_link, class_of, find_final_vae_decode_id, inputs, link, set_input};
use crate::env::{GraphEnv, GraphError, Result};

/// The node that loads a scene's whole saved latent.
pub const LOADER_CLASS: &str = "VRGDG_MiniMaxH3LoadLatentFull";
pub const DEFAULT_CRF: u32 = 12;
pub const MAX_CRF: u32 = 30;
const _: () = assert!(DEFAULT_CRF < 19, "the default must be finer than the CRF 19 render");
const AUDIO_DRIVE_CLASS: &str = "VRGDG_MiniMaxH3AudioDrive";
/// The sockets a decode node reads the sampled latent from.
const LATENT_INPUTS: [&str; 3] = ["samples", "av_latent", "latent"];

/// The CRF for a re-decode: 12 when not given, else a whole number from 0 to 30.
pub fn parse_crf(value: Option<&Value>) -> Result<u32> {
    let value = match value {
        None | Some(Value::Null) => return Ok(DEFAULT_CRF),
        Some(Value::String(text)) if text.trim().is_empty() => return Ok(DEFAULT_CRF),
        Some(value) => value,
    };
    let number = match value {
        Value::Number(n) => n.as_f64(),
        Value::String(text) => text.trim().parse::<f64>().ok(),
        _ => None,
    }
    .filter(|n| n.is_finite());
    let Some(number) = number else {
        return Err(GraphError::new(format!("crf must be a whole number from 0 to {MAX_CRF}, got {value}.")));
    };
    let crf = number.trunc() as i64;
    if !(0..=i64::from(MAX_CRF)).contains(&crf) {
        return Err(GraphError::new(format!("crf must be from 0 to {MAX_CRF}, got {crf}.")));
    }
    Ok(crf as u32)
}

/// The ids of `root` and every node it reads from, directly or through other nodes.
fn reachable(prompt: &Obj, root: &str) -> HashSet<String> {
    let mut keep = HashSet::new();
    let mut todo = vec![root.to_string()];
    while let Some(id) = todo.pop() {
        if keep.contains(&id) || !prompt.contains_key(&id) {
            continue;
        }
        if let Some(ins) = inputs(prompt, &id) {
            todo.extend(ins.values().filter_map(as_link).map(|(next, _)| next));
        }
        keep.insert(id);
    }
    keep
}

/// The node that writes the scene video: the one the builder named, else the first video combine.
fn final_video_node(prompt: &Obj, graph: &Obj) -> Result<String> {
    if let Some(named) = graph.get("final_video_node_id").and_then(Value::as_str).filter(|id| prompt.contains_key(*id)) {
        return Ok(named.to_string());
    }
    if let Some(id) = ["353", "142"].iter().find(|id| class_of(prompt, id) == "VHS_VideoCombine") {
        return Ok((*id).to_string());
    }
    prompt
        .iter()
        .find(|(_, node)| node.get("class_type").and_then(Value::as_str) == Some("VHS_VideoCombine"))
        .map(|(id, _)| id.clone())
        .ok_or_else(|| GraphError::new("The scene graph has no video output node to re-decode into."))
}

/// Cut a compiled scene graph down to "load the saved latent, decode, resize, encode at `crf`".
///
/// `graph` is what the MiniMax graph builders return. The result has the same shape with the smaller prompt, so the trim and
/// collect steps work as they do after a render. It refuses a scene that cannot be re-decoded: no saved latent, a latent that
/// does not match the scene's current settings, built-in audio, or a graph without a decode or a video output.
pub fn build_redecode_graph(env: &dyn GraphEnv, graph: &Obj, project: &str, scene: u32, crf: u32) -> Result<Obj> {
    if graph.get("audio_mode").and_then(Value::as_str) == Some("built_in_audio") {
        return Err(GraphError::new(
            "This scene uses built-in audio, which is decoded from the first-pass latent. Re-decoding supports scenes that play the song or another audio file.",
        ));
    }
    let mut prompt: Obj = graph.get("prompt").and_then(Value::as_object).cloned().unwrap_or_default();
    let video_node = final_video_node(&prompt, graph)?;
    let no_decode = || GraphError::new("The scene graph has no VAE decode to feed from a saved latent.");
    let decode_id = find_final_vae_decode_id(&prompt).map_err(|_| no_decode())?;
    let latent_input = LATENT_INPUTS
        .iter()
        .find(|name| inputs(&prompt, &decode_id).and_then(|ins| ins.get(**name)).and_then(as_link).is_some())
        .ok_or_else(no_decode)?;

    let facts = env.latent_facts(project, scene);
    if !facts.exists {
        return Err(GraphError::new(format!("Scene {scene} has no saved latent. Render the scene first; MiniMax H3 renders save one.")));
    }
    let saved = tokens_to_frames(facts.token_count);
    let expected = graph.get("timing").and_then(|timing| timing.get("h3_frame_count")).and_then(Value::as_i64).unwrap_or(0);
    if expected > 0 && saved > 0 && expected != saved {
        return Err(GraphError::new(format!(
            "Scene {scene}'s saved latent has {saved} frames, but its current settings and timing call for {expected}. The scene was changed after it was rendered, so the latent cannot be trimmed to the scene exactly. Render the scene again."
        )));
    }

    let loader_id = (prompt.keys().filter_map(|key| key.parse::<i64>().ok()).max().unwrap_or(0).max(9000) + 1).to_string();
    prompt.insert(
        loader_id.clone(),
        json!({
            "class_type": LOADER_CLASS,
            "inputs": {"project_folder": project, "scene_number": scene},
            "_meta": {"title": format!("Saved Latent (Scene {scene:03})")},
        }),
    );
    set_input(&mut prompt, &decode_id, latent_input, link(&loader_id, 0))?;

    // The video is muxed with the source audio file, not with anything sampled. The audio drive only passes that file through
    // (and needs the text encoder and reference nodes to build its latent), so read the file directly.
    let audio = inputs(&prompt, &video_node).and_then(|ins| ins.get("audio")).and_then(as_link);
    if let Some((drive, 1)) = audio.filter(|(id, _)| class_of(&prompt, id) == AUDIO_DRIVE_CLASS) {
        let source = inputs(&prompt, &drive).and_then(|ins| ins.get("source_audio")).filter(|value| as_link(value).is_some()).cloned();
        if let Some(source) = source {
            set_input(&mut prompt, &video_node, "audio", source)?;
        }
    }

    let keep = reachable(&prompt, &video_node);
    prompt.retain(|id, _| keep.contains(id));
    set_input(&mut prompt, &video_node, "crf", json!(crf))?;

    let mut result = graph.clone();
    let node_count = prompt.len();
    result.insert("prompt".into(), Value::Object(prompt));
    result.insert("final_video_node_id".into(), json!(video_node));
    result.insert("redecode".into(), json!({"crf": crf, "frame_count": saved, "latent_path": env.latent_path(project, scene), "node_count": node_count}));
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::env::LatentFacts;
    use h3_core::timing::TimingPlan;

    struct Env(LatentFacts);
    impl GraphEnv for Env {
        fn is_file(&self, _: &str) -> bool { false }
        fn is_dir(&self, _: &str) -> bool { true }
        fn ensure_dir(&self, _: &str) {}
        fn node_info(&self, _: &str) -> Option<Value> { None }
        fn model_choices(&self, _: &str) -> Vec<String> { vec![] }
        fn output_dir(&self) -> String { String::new() }
        fn probe_duration(&self, _: &str) -> std::result::Result<f64, String> { Ok(10.0) }
        fn trim_audio(&self, _: &str, _: &str, _: u32, _: &TimingPlan) -> std::result::Result<Value, String> { Err("unused".into()) }
        fn latent_exists(&self, _: &str, _: u32) -> bool { self.0.exists }
        fn latent_path(&self, project: &str, scene: u32) -> String { format!("{project}/latents/scene_{scene:03}.latent") }
        fn latent_facts(&self, _: &str, _: u32) -> LatentFacts { self.0.clone() }
        fn pass1_latent_matches(&self, _: &str, _: u32) -> bool { false }
        fn random_seed(&self) -> u64 { 1 }
    }

    const TOKENS: i64 = 45;

    /// The frames the saved latent holds, which is what the scene's timing has to ask for.
    fn frames() -> i64 {
        tokens_to_frames(TOKENS)
    }

    fn env() -> Env {
        Env(LatentFacts { exists: true, token_count: TOKENS, tail_padding_frames: None })
    }

    /// The shape of a compiled two pass scene as the render leaves it: the save nodes in front of the decode, the sampler chain,
    /// the text encoder, the audio drive, and the video combine that reads the song through the drive.
    fn graph() -> Obj {
        json!({
            "audio_mode": "input_audio",
            "timing": {"h3_frame_count": frames()},
            "post_render_trim": {"start": 0.0, "duration": 6.0, "frames": 144},
            "prompt": {
                "115": {"class_type": "PrimitiveInt", "inputs": {"value": 1280}},
                "119": {"class_type": "VAELoader", "inputs": {"vae_name": "video.safetensors"}},
                "120": {"class_type": "VAELoader", "inputs": {"vae_name": "audio.safetensors"}},
                "122": {"class_type": "VAEDecode", "inputs": {"samples": ["9850", 0], "vae": ["119", 0]}},
                "128": {"class_type": "CLIPLoader", "inputs": {"clip_name": "text.safetensors"}},
                "136": {"class_type": "MiniMaxH3ReferenceToVideo", "inputs": {"clip": ["128", 0], "vae": ["119", 0]}},
                "142": {"class_type": "VHS_VideoCombine", "inputs": {"images": ["183", 0], "audio": ["172", 1], "crf": 19, "filename_prefix": "scene"}},
                "171": {"class_type": "VHS_LoadAudio", "inputs": {"audio_file": "song.wav"}},
                "172": {"class_type": "VRGDG_MiniMaxH3AudioDrive", "inputs": {"av_latent": ["136", 1], "source_audio": ["171", 0], "audio_vae": ["120", 0]}},
                "183": {"class_type": "ImageResizeKJv2", "inputs": {"width": ["115", 0], "height": ["184", 0], "image": ["122", 0]}},
                "184": {"class_type": "PrimitiveInt", "inputs": {"value": 736}},
                "194": {"class_type": "SamplerCustomAdvanced", "inputs": {"latent_image": ["136", 1]}},
                "9850": {"class_type": "VRGDG_MiniMaxH3SaveLatent", "inputs": {"latent": ["194", 0], "scene_number": 1}},
                "9851": {"class_type": "VRGDG_MiniMaxH3SaveLatent", "inputs": {"latent": ["194", 0], "scene_number": 1, "pass1": true}},
            }
        })
        .as_object()
        .cloned()
        .unwrap()
    }

    fn classes(result: &Obj) -> Vec<String> {
        let mut classes: Vec<String> = result["prompt"].as_object().unwrap().values().map(|n| n["class_type"].as_str().unwrap().to_string()).collect();
        classes.sort();
        classes
    }

    #[test]
    fn the_graph_is_load_decode_resize_encode() {
        let result = build_redecode_graph(&env(), &graph(), "C:/p", 1, 12).unwrap();
        assert_eq!(classes(&result), vec!["ImageResizeKJv2", "PrimitiveInt", "PrimitiveInt", "VAEDecode", "VAELoader", "VHS_LoadAudio", "VHS_VideoCombine", LOADER_CLASS]);
        let prompt = result["prompt"].as_object().unwrap();
        let loader = prompt["122"]["inputs"]["samples"][0].as_str().unwrap();
        assert_eq!(prompt[loader]["class_type"], LOADER_CLASS);
        assert_eq!(prompt[loader]["inputs"], json!({"project_folder": "C:/p", "scene_number": 1}));
        assert_eq!(result["final_video_node_id"], "142");
        // The trim and timing of the render are kept for the steps after it.
        assert_eq!(result["post_render_trim"]["frames"], 144);
        assert_eq!(result["redecode"]["frame_count"], frames());
    }

    #[test]
    fn nothing_that_samples_saves_or_encodes_text_stays() {
        let result = build_redecode_graph(&env(), &graph(), "C:/p", 1, 12).unwrap();
        let classes = classes(&result);
        for gone in ["SamplerCustomAdvanced", "VRGDG_MiniMaxH3SaveLatent", "CLIPLoader", "MiniMaxH3ReferenceToVideo", "VRGDG_MiniMaxH3AudioDrive"] {
            assert!(!classes.iter().any(|c| c == gone), "{gone} stayed");
        }
    }

    #[test]
    fn the_encoder_gets_the_chosen_crf_and_the_song_without_the_audio_drive() {
        for crf in [0, 12, 30] {
            let result = build_redecode_graph(&env(), &graph(), "C:/p", 1, crf).unwrap();
            let video = &result["prompt"]["142"]["inputs"];
            assert_eq!(video["crf"], crf);
            assert_eq!(video["audio"], json!(["171", 0]));
        }
    }

    #[test]
    fn every_link_points_at_a_node_that_is_still_there() {
        let result = build_redecode_graph(&env(), &graph(), "C:/p", 1, 12).unwrap();
        let prompt = result["prompt"].as_object().unwrap();
        for node in prompt.values() {
            for value in node["inputs"].as_object().unwrap().values() {
                if let Some((id, _)) = as_link(value) {
                    assert!(prompt.contains_key(&id), "link to {id}");
                }
            }
        }
    }

    #[test]
    fn the_compiled_graph_is_not_changed() {
        let before = graph();
        let _ = build_redecode_graph(&env(), &before, "C:/p", 1, 12).unwrap();
        assert_eq!(before, graph());
    }

    #[test]
    fn a_scene_that_cannot_be_decoded_again_is_refused() {
        let none = Env(LatentFacts::default());
        assert!(build_redecode_graph(&none, &graph(), "C:/p", 1, 12).unwrap_err().message.contains("no saved latent"));

        let mut moved = graph();
        moved["timing"] = json!({"h3_frame_count": 175});
        let message = build_redecode_graph(&env(), &moved, "C:/p", 1, 12).unwrap_err().message;
        assert!(message.contains(&format!("{} frames", frames())) && message.contains("call for 175"), "{message}");

        let mut built_in = graph();
        built_in["audio_mode"] = json!("built_in_audio");
        assert!(build_redecode_graph(&env(), &built_in, "C:/p", 1, 12).unwrap_err().message.contains("built-in audio"));

        let mut no_output = graph();
        no_output["prompt"].as_object_mut().unwrap().remove("142");
        assert!(build_redecode_graph(&env(), &no_output, "C:/p", 1, 12).unwrap_err().message.contains("video output"));
    }

    #[test]
    fn crf_is_read_like_the_pack_does() {
        assert_eq!(parse_crf(None).unwrap(), DEFAULT_CRF);
        assert_eq!(parse_crf(Some(&Value::Null)).unwrap(), DEFAULT_CRF);
        assert_eq!(parse_crf(Some(&json!(""))).unwrap(), DEFAULT_CRF);
        assert_eq!(parse_crf(Some(&json!("10"))).unwrap(), 10);
        assert_eq!(parse_crf(Some(&json!(12.9))).unwrap(), 12);
        assert_eq!(parse_crf(Some(&json!(0))).unwrap(), 0);
        assert_eq!(parse_crf(Some(&json!(MAX_CRF))).unwrap(), MAX_CRF);
        for bad in [json!(-1), json!(MAX_CRF + 1), json!("fine"), json!([12]), json!(true)] {
            assert!(parse_crf(Some(&bad)).is_err(), "{bad}");
        }
    }
}
