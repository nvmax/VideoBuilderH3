//! Project routes of the Builder: list, open, save, create and delete.

use std::sync::Arc;

use axum::Router;
use h3_builder::payload::path_text;
use h3_builder::projects;
use h3_project::Stamp;

use super::{get_query, post, Fail, Out};
use crate::app::AppState;

pub fn routes(router: Router<Arc<AppState>>) -> Router<Arc<AppState>> {
    router
        .route("/vrgdg/music_builder/list_projects", get_query(|env, query| Ok(Out::from(projects::list_projects(&env, &path_text(&query, "project_root"))))))
        .route("/vrgdg/music_builder/load_session", post(|env, payload| projects::load_session(&env, &payload).map(Out::from).map_err(Fail::from)))
        .route("/vrgdg/music_builder/save_session", post(|env, payload| projects::save_session(&env, &payload, &Stamp::now()).map(Out::from).map_err(Fail::from)))
        .route("/vrgdg/music_builder/new_project", post(|env, payload| projects::new_project(&env, &payload, &Stamp::now()).map(Out::from).map_err(Fail::from)))
        .route("/vrgdg/music_builder/delete_project", post(|env, payload| projects::delete_project(&env, &payload).map(Out::from).map_err(Fail::from)))
}
