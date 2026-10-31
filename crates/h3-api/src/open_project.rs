//! The project the app's window has open. The window tells the server (`PUT /app/open-project`) whenever its Builder opens another project, so the server can
//! tell an agent: in the instructions it gets when it connects (`mcp/mod.rs`) and on request (`GET /app/open-project`). Only a project the agent API can
//! address counts, one with a saved session in a project folder the app lists.

use std::path::PathBuf;
use std::sync::Mutex;

use axum::extract::State;
use axum::Json;
use serde_json::{json, Value};

use crate::app::{body_object, AppState, Shared};
use crate::error::{success, ApiResult};
use crate::views;

/// The tool an agent calls to ask again. The instructions name it, and a test keeps the name true.
pub const OPEN_PROJECT_TOOL: &str = "api_get_app_open_project";

/// The id (folder name) of the project the window said it has open.
#[derive(Default)]
pub struct OpenProject(Mutex<Option<String>>);

/// The last part of a folder path, whichever way the slashes lean.
fn id_of(folder: &str) -> &str {
    folder.rsplit(|c| c == '/' || c == '\\').find(|part| !part.is_empty()).unwrap_or("")
}

impl AppState {
    /// The window says which project it has open (`folder` is its path, empty for none). A project the agent API cannot find counts as none.
    pub fn set_open_project(&self, folder: &str) {
        let id = id_of(folder.trim());
        let found = (!id.is_empty() && views::resolve_project(&self.roots, id).is_ok()).then(|| id.to_string());
        *self.open_project.0.lock().unwrap_or_else(|e| e.into_inner()) = found;
    }

    /// The open project's id and folder. None when no project is open or it was deleted since.
    pub fn open_project(&self) -> Option<(String, PathBuf)> {
        let id = self.open_project.0.lock().unwrap_or_else(|e| e.into_inner()).clone()?;
        let folder = views::resolve_project(&self.roots, &id).ok()?;
        Some((id, folder))
    }

    fn open_project_view(&self) -> Value {
        match self.open_project() {
            Some((id, folder)) => json!({"open": true, "project_id": id, "project_folder": folder.display().to_string()}),
            None => json!({"open": false, "project_id": null, "project_folder": null}),
        }
    }

    /// What the instructions an agent gets when it connects say about the open project.
    pub fn open_project_note(&self) -> String {
        match self.open_project() {
            Some((id, _)) => format!(
                "The project open in the app right now is {id:?}. When the user says this project, the current project or the open project, that is the one: use it as project_id. \
                 The user can open another at any time, so call `{OPEN_PROJECT_TOOL}` to check again before you act on it."
            ),
            None => format!("No project is open in the app right now. When the user means this project, call `{OPEN_PROJECT_TOOL}` to check again, or ask which one."),
        }
    }
}

/// `GET /app/open-project`: the project open in the app's window.
pub async fn get_open(State(state): Shared) -> ApiResult {
    Ok(success(state.open_project_view(), None))
}

/// `PUT /app/open-project`: the window says which project it has open. Body: `project_folder`, empty when none is open.
pub async fn put_open(State(state): Shared, body: Option<Json<Value>>) -> ApiResult {
    let body = body_object(body)?;
    state.set_open_project(body.get("project_folder").and_then(Value::as_str).unwrap_or(""));
    Ok(success(state.open_project_view(), None))
}

#[cfg(test)]
mod tests {
    use super::id_of;

    #[test]
    fn the_id_is_the_last_part_of_the_path() {
        assert_eq!(id_of(r"C:\ComfyUI\output\My Song"), "My Song");
        assert_eq!(id_of("/home/me/output/My Song/"), "My Song");
        assert_eq!(id_of("Demo"), "Demo");
        assert_eq!(id_of(""), "");
        assert_eq!(id_of(r"\\"), "");
    }
}
