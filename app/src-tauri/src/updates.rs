//! Updates. The app looks for a newer installer on the GitHub release page of the project and installs it when the user agrees.
//! The window asks through three commands: `update_check` finds out, `update_install` downloads and installs, `update_progress` says how far
//! the download is. The installer only runs when its signature matches the public key built into the app, and nothing is installed while a job
//! (a render, a prompt run, a lyric alignment) is running or waiting.

use std::sync::{Mutex, MutexGuard};

use h3_jobs::model::{QUEUED, RUNNING};
use h3_jobs::JobManager;
use serde::Serialize;
use tauri::{AppHandle, State};
use tauri_plugin_updater::{Update, Updater, UpdaterExt};

#[derive(Clone, Copy, Default, Serialize, PartialEq)]
#[serde(rename_all = "lowercase")]
enum Phase {
    #[default]
    Idle,
    Downloading,
    Installing,
    Failed,
}

#[derive(Clone, Default, Serialize)]
pub struct Progress {
    phase: Phase,
    downloaded: u64,
    total: Option<u64>,
    error: Option<String>,
}

/// The update the last check found, and how far its download is.
#[derive(Default)]
pub struct Updates {
    found: Mutex<Option<Update>>,
    progress: Mutex<Progress>,
}

#[derive(Serialize)]
pub struct UpdateInfo {
    version: String,
    current_version: String,
    notes: Option<String>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

fn updater(app: &AppHandle) -> Result<Updater, String> {
    let mut builder = app.updater_builder();
    // A debug build can look at another address, so the window can be tried without a published release. Its answer still has to be signed.
    if cfg!(debug_assertions) {
        if let Ok(address) = std::env::var("H3_UPDATE_ENDPOINT") {
            let address: tauri::Url = address.parse().map_err(|e| format!("H3_UPDATE_ENDPOINT is not an address: {e}"))?;
            builder = builder.endpoints(vec![address]).map_err(|e| e.to_string())?;
        }
    }
    builder.build().map_err(|e| e.to_string())
}

/// Jobs that run or wait. An update restarts the app, which would stop them.
fn open_jobs(jobs: &JobManager) -> Result<usize, String> {
    let mut count = 0;
    for status in [RUNNING, QUEUED] {
        count += jobs.list(None, Some(status), None).map_err(|e| e.to_string())?.len();
    }
    Ok(count)
}

fn refuse_while_busy(jobs: &JobManager) -> Result<(), String> {
    match open_jobs(jobs)? {
        0 => Ok(()),
        1 => Err("A job is still running. Install the update when it has finished, or cancel it first.".into()),
        n => Err(format!("{n} jobs are still running or waiting. Install the update when they have finished, or cancel them first.")),
    }
}

fn set_failed(updates: &Updates, error: String) -> String {
    let mut progress = lock(&updates.progress);
    *progress = Progress { phase: Phase::Failed, error: Some(error.clone()), ..Progress::default() };
    error
}

/// The version of the running app.
#[tauri::command]
pub fn app_version(app: AppHandle) -> String {
    app.package_info().version.to_string()
}

/// Ask GitHub whether a newer version exists. Nothing is downloaded.
#[tauri::command]
pub async fn update_check(app: AppHandle, updates: State<'_, Updates>) -> Result<Option<UpdateInfo>, String> {
    let found = updater(&app)?.check().await.map_err(|e| format!("The update check failed: {e}"))?;
    let info = found.as_ref().map(|update| UpdateInfo {
        version: update.version.clone(),
        current_version: update.current_version.clone(),
        notes: update.body.clone().filter(|notes| !notes.trim().is_empty()),
    });
    *lock(&updates.found) = found;
    Ok(info)
}

/// How far the download is, for the window to show.
#[tauri::command]
pub fn update_progress(updates: State<'_, Updates>) -> Progress {
    lock(&updates.progress).clone()
}

/// Download the update the last check found, check its signature and run the installer. The app closes while the installer runs and starts
/// again when it is done.
#[tauri::command]
pub async fn update_install(app: AppHandle, updates: State<'_, Updates>, jobs: State<'_, JobManager>) -> Result<(), String> {
    refuse_while_busy(&jobs)?;
    let Some(update) = lock(&updates.found).clone() else {
        return Err("Check for updates first.".into());
    };
    if lock(&updates.progress).phase == Phase::Downloading {
        return Err("The update is already downloading.".into());
    }
    *lock(&updates.progress) = Progress { phase: Phase::Downloading, ..Progress::default() };
    let bytes = update
        .download(
            |chunk, total| {
                let mut progress = lock(&updates.progress);
                progress.downloaded += chunk as u64;
                progress.total = total;
            },
            || {},
        )
        .await
        .map_err(|e| set_failed(&updates, format!("The update could not be downloaded: {e}")))?;
    // A job can start while the file downloads, so this is asked again right before the installer runs.
    refuse_while_busy(&jobs).map_err(|e| set_failed(&updates, e))?;
    lock(&updates.progress).phase = Phase::Installing;
    update.install(bytes).map_err(|e| set_failed(&updates, format!("The update could not be installed: {e}")))?;
    // Windows closes the app when the installer starts. Other systems get here and start again.
    app.restart()
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use h3_jobs::{JobError, JobStore};
    use serde_json::json;

    use super::*;

    async fn wait_until(what: &str, done: impl Fn() -> bool) {
        for _ in 0..200 {
            if done() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        panic!("{what} did not happen");
    }

    #[tokio::test]
    async fn an_update_waits_for_running_and_queued_jobs() {
        let jobs = JobManager::new(JobStore::open_in_memory().unwrap());
        jobs.register(
            "slow",
            Arc::new(|ctx| {
                Box::pin(async move {
                    while !ctx.is_cancelled() {
                        tokio::time::sleep(Duration::from_millis(10)).await;
                    }
                    Err(JobError::cancelled())
                })
            }),
        );
        assert_eq!(refuse_while_busy(&jobs), Ok(()));

        // Two GPU jobs: one runs and the other waits behind it.
        let first = jobs.submit("slow", None, json!({}), true).unwrap();
        wait_until("the first job starting", || jobs.get(&first.id).unwrap().status == "running").await;
        assert!(refuse_while_busy(&jobs).unwrap_err().starts_with("A job is still running"));
        let second = jobs.submit("slow", None, json!({}), true).unwrap();
        assert!(refuse_while_busy(&jobs).unwrap_err().starts_with("2 jobs are still running or waiting"));

        for id in [&second.id, &first.id] {
            jobs.cancel(id).unwrap();
        }
        wait_until("both jobs ending", || [&first.id, &second.id].iter().all(|id| jobs.get(id).unwrap().is_terminal())).await;
        assert_eq!(refuse_while_busy(&jobs), Ok(()), "finished jobs do not hold an update back");
    }
}
