//! Settings routes of the Builder: model defaults, the LLM runner, profiles and presets.

use std::sync::Arc;

use axum::http::StatusCode;
use axum::routing::MethodRouter;
use axum::Router;
use h3_builder::payload::text;
use h3_core::pyval::{text_or_empty, truthy, Obj};
use h3_project::builder_store::{self, AdjustPresets, SceneDefaultsPresets, StoreError, UiProfiles, VideoProfiles};
use h3_project::Stamp;
use serde_json::{json, Value};

use super::{get_query, post, Fail, Out, Reply};
use crate::app::AppState;

fn name_of(payload: &Obj) -> String {
    text_or_empty(payload.get("name"))
}

fn overwrite(payload: &Obj) -> bool {
    payload.get("overwrite").is_some_and(truthy)
}

/// A Scene Defaults preset that is not found answers with its own message, as the pack does.
fn preset_error(error: StoreError) -> Fail {
    match error {
        StoreError::NotFound(_) => Fail::new(StatusCode::NOT_FOUND, "Scene Defaults preset was not found."),
        StoreError::Exists { message, .. } => Fail::new(StatusCode::CONFLICT, message),
        other => other.into(),
    }
}

fn io_fail(error: std::io::Error) -> Fail {
    Fail::from(error.to_string())
}

pub fn routes(router: Router<Arc<AppState>>) -> Router<Arc<AppState>> {
    let router = router
        .route("/vrgdg/music_builder/model_defaults", get_query(|env, _| Ok(builder_store::load_model_defaults(&env.output)?.into())))
        .route("/vrgdg/music_builder/llm_runner", post(|env, payload| builder_store::save_global_llm_settings(&env.output, &payload).map(Out::from).map_err(io_fail)))
        .route("/vrgdg/workflow_runner/model_root", get_query(|env, _| Ok(builder_store::load_custom_model_root(&env.output).into())).merge(post_model_root()));
    let router = video_profile_routes(router);
    let router = ui_profile_routes(router);
    let router = scene_defaults_routes(router);
    adjust_preset_routes(router)
}

fn post_model_root() -> MethodRouter<Arc<AppState>> {
    post(|env, payload| builder_store::save_custom_model_root(&env.output, &text(&payload, "models_root")).map(Out::from).map_err(io_fail))
}

fn video_profile_routes(router: Router<Arc<AppState>>) -> Router<Arc<AppState>> {
    router
        .route(
            "/vrgdg/music_builder/list_video_profiles",
            post(|env, _| {
                let profiles = VideoProfiles::new(&env.output);
                Ok(json!({"profiles": profiles.list(), "last": profiles.last()}).into())
            }),
        )
        .route(
            "/vrgdg/music_builder/load_video_profile",
            post(|env, payload| {
                let profiles = VideoProfiles::new(&env.output);
                let profile = profiles.load(&name_of(&payload))?;
                profiles.set_last(&text_or_empty(profile.get("name")))?;
                Ok(json!({"profile": profile}).into())
            }),
        )
        .route("/vrgdg/music_builder/set_last_video_profile", post(|env, payload| Ok(json!({"name": VideoProfiles::new(&env.output).set_last(&name_of(&payload))?}).into())))
        .route(
            "/vrgdg/music_builder/save_video_profile",
            post(|env, payload| {
                let settings = payload.get("settings").cloned().unwrap_or(Value::Null);
                let profile = VideoProfiles::new(&env.output).save(&name_of(&payload), &settings, overwrite(&payload), &Stamp::now())?;
                Ok(json!({"profile": profile}).into())
            }),
        )
        .route("/vrgdg/music_builder/delete_video_profile", post(|env, payload| Ok(VideoProfiles::new(&env.output).delete(&name_of(&payload))?.into())))
}

fn ui_profile_routes(router: Router<Arc<AppState>>) -> Router<Arc<AppState>> {
    router
        .route(
            "/vrgdg/music_builder/list_ui_profiles",
            post(|env, _| {
                let profiles = UiProfiles::new(&env.output);
                Ok(json!({"profiles": profiles.list(), "last": profiles.last()}).into())
            }),
        )
        .route("/vrgdg/music_builder/load_default_ui_layout", post(|env, _| Ok(json!({"layout": UiProfiles::new(&env.output).default_layout()}).into())))
        .route("/vrgdg/music_builder/save_default_ui_layout", post(|env, payload| Ok(json!({"layout": UiProfiles::new(&env.output).save_default_layout(payload.get("layout"))?}).into())))
        .route(
            "/vrgdg/music_builder/load_ui_profile",
            post(|env, payload| {
                let profiles = UiProfiles::new(&env.output);
                let profile = profiles.load(&name_of(&payload))?;
                profiles.set_last(&text_or_empty(profile.get("name")))?;
                Ok(json!({"profile": profile}).into())
            }),
        )
        .route("/vrgdg/music_builder/set_last_ui_profile", post(|env, payload| Ok(json!({"name": UiProfiles::new(&env.output).set_last(&name_of(&payload))?}).into())))
        .route(
            "/vrgdg/music_builder/save_ui_profile",
            post(|env, payload| Ok(json!({"profile": UiProfiles::new(&env.output).save(&name_of(&payload), payload.get("layout"), overwrite(&payload), &Stamp::now())?}).into())),
        )
        .route(
            "/vrgdg/music_builder/update_ui_profile_layout",
            post(|env, payload| Ok(json!({"profile": UiProfiles::new(&env.output).update_layout(&name_of(&payload), payload.get("layout"), &Stamp::now())?}).into())),
        )
        .route("/vrgdg/music_builder/delete_ui_profile", post(|env, payload| Ok(UiProfiles::new(&env.output).delete(&name_of(&payload))?.into())))
}

fn scene_defaults_routes(router: Router<Arc<AppState>>) -> Router<Arc<AppState>> {
    router
        .route("/vrgdg/music_builder/list_scene_defaults_presets", post(|env, _| Ok(json!({"presets": SceneDefaultsPresets::new(&env.output).list()}).into())))
        .route(
            "/vrgdg/music_builder/load_scene_defaults_preset",
            post(|env, payload| Ok(json!({"preset": SceneDefaultsPresets::new(&env.output).load(&name_of(&payload)).map_err(preset_error)?}).into())),
        )
        .route(
            "/vrgdg/music_builder/save_scene_defaults_preset",
            post(|env, payload| {
                let settings = payload.get("settings").cloned().unwrap_or(Value::Null);
                // The pack replaces a preset only for `overwrite is True`, not for any truthy value.
                let replace = payload.get("overwrite") == Some(&json!(true));
                Ok(json!({"preset": SceneDefaultsPresets::new(&env.output).save(&name_of(&payload), &settings, replace).map_err(preset_error)?}).into())
            }),
        )
}

fn adjust_preset_routes(router: Router<Arc<AppState>>) -> Router<Arc<AppState>> {
    fn with_list(env: &h3_builder::Env, preset: Value) -> Reply {
        let mut out = AdjustPresets::new(&env.output).list();
        if let Some(map) = out.as_object_mut() {
            let mut merged = serde_json::Map::new();
            merged.insert("preset".into(), preset);
            merged.extend(std::mem::take(map));
            *map = merged;
        }
        Ok(Out::Fields(out))
    }
    router
        .route("/vrgdg/music_builder/post_process/adjust/presets", get_query(|env, _| Ok(AdjustPresets::new(&env.output).list().into())))
        .route(
            "/vrgdg/music_builder/post_process/adjust/presets/save",
            post(|env, payload| {
                let preset = AdjustPresets::new(&env.output).save(&text_or_empty(payload.get("name")), payload.get("settings"), &Stamp::now())?;
                with_list(&env, preset)
            }),
        )
        .route(
            "/vrgdg/music_builder/post_process/adjust/presets/import",
            post(|env, payload| {
                let fallback = payload.get("name").map_or_else(|| "Imported Adjust Preset".to_string(), |v| text_or_empty(Some(v)));
                let preset = AdjustPresets::new(&env.output).import(payload.get("preset"), &fallback, &Stamp::now())?;
                with_list(&env, preset)
            }),
        )
}
