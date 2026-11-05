//! Signed desktop updates. Frontend input is limited to a stable release tag.
#[cfg(any(windows, test))]
mod replacement;
mod signature;
#[cfg(windows)]
mod windows;

use parking_lot::{Mutex, MutexGuard};
use serde::Serialize;
use tauri::{Emitter, Manager, State};
use tauri_plugin_updater::{Update, UpdaterExt};

const PUBLIC_KEY: Option<&str> = option_env!("BYTETIDE_UPDATER_PUBLIC_KEY");
#[derive(Default)]
pub struct DesktopUpdater(Mutex<UpdateState>, Mutex<bool>);

impl DesktopUpdater {
    /// Hold this permit until a new live session, replay, or scenario is registered.
    /// Downloads do not block monitoring; only installation closes this gate.
    pub(crate) fn start_guard(&self) -> Result<MutexGuard<'_, bool>, String> {
        let guard = self.1.lock();
        if *guard {
            return Err(failure("application is shutting down to install an update"));
        }
        Ok(guard)
    }

    #[cfg(any(windows, target_os = "macos", test))]
    fn begin_shutdown(&self) {
        // Waiting for this lock drains startups that already have a permit.
        *self.1.lock() = true;
    }

    #[cfg(any(target_os = "macos", test))]
    fn reopen_starts(&self) {
        *self.1.lock() = false;
    }
}
#[derive(Default)]
struct UpdateState {
    busy: bool,
    pending: Option<Pending>,
}
struct Pending {
    update: Update,
    bytes: Vec<u8>,
    #[cfg(windows)]
    manifest: Vec<u8>,
    #[cfg(windows)]
    manifest_signature: String,
}
#[derive(Serialize)]
pub struct UpdateInfo {
    supported: bool,
}
#[derive(Serialize)]
pub struct Downloaded {
    version: String,
}
#[derive(Clone, Serialize)]
struct Progress {
    downloaded: u64,
    total: Option<u64>,
}
fn failure(e: impl std::fmt::Display) -> String {
    format!("desktop_update_failed|{e}")
}
fn key() -> Result<&'static str, String> {
    PUBLIC_KEY
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| failure("updater signing key is not configured"))
}
fn stable_version(tag: &str) -> Result<semver::Version, String> {
    let raw = tag
        .strip_prefix('v')
        .ok_or_else(|| failure("invalid desktop release tag"))?;
    let version = semver::Version::parse(raw).map_err(failure)?;
    if !version.pre.is_empty() || !version.build.is_empty() || version.to_string() != raw {
        return Err(failure("expected a canonical stable desktop release tag"));
    }
    Ok(version)
}
async fn bounded_body(
    client: &reqwest::Client,
    url: &str,
    limit: usize,
) -> Result<Vec<u8>, String> {
    let mut response = client
        .get(url)
        .send()
        .await
        .map_err(failure)?
        .error_for_status()
        .map_err(failure)?;
    if response
        .content_length()
        .is_some_and(|size| size > limit as u64)
    {
        return Err(failure("update metadata exceeds size limit"));
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(failure)? {
        if chunk.len() > limit.saturating_sub(bytes.len()) {
            return Err(failure("update metadata exceeds size limit"));
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}
#[tauri::command]
pub fn desktop_update_info_cmd() -> UpdateInfo {
    UpdateInfo {
        supported: cfg!(any(windows, target_os = "macos")) && key().is_ok(),
    }
}
#[tauri::command]
pub async fn desktop_update_download_cmd(
    app: tauri::AppHandle,
    state: State<'_, DesktopUpdater>,
    tag: String,
) -> Result<Downloaded, String> {
    if !desktop_update_info_cmd().supported {
        return Err(failure("desktop updating is unavailable"));
    }
    let version = stable_version(&tag)?;
    if version
        <= semver::Version::parse(&app.package_info().version.to_string()).map_err(failure)?
    {
        return Err(failure("update must be newer than the installed version"));
    }
    {
        let mut s = state.0.lock();
        if s.busy {
            return Err(failure("another update operation is running"));
        }
        s.busy = true;
        s.pending = None;
    }
    let result = async {
        let endpoint_url =
            format!("https://github.com/RtuQ/bytetide/releases/download/{tag}/desktop-update.json");
        let client = reqwest::Client::builder()
            .https_only(true)
            .timeout(std::time::Duration::from_secs(60))
            .build()
            .map_err(failure)?;
        let manifest = bounded_body(&client, &endpoint_url, 1024 * 1024).await?;
        let manifest_signature = String::from_utf8(
            bounded_body(&client, &format!("{endpoint_url}.sig"), 16 * 1024).await?,
        )
        .map_err(failure)?;
        signature::verify(&manifest, manifest_signature.trim(), key()?).map_err(failure)?;
        let authenticated: serde_json::Value =
            serde_json::from_slice(&manifest).map_err(failure)?;
        let endpoint = endpoint_url.parse().map_err(failure)?;
        let platform = format!(
            "{}-{}",
            if cfg!(windows) { "windows" } else { "darwin" },
            std::env::consts::ARCH
        );
        let updater = app
            .updater_builder()
            .target(platform)
            .pubkey(key()?)
            .timeout(std::time::Duration::from_secs(300))
            .endpoints(vec![endpoint])
            .map_err(failure)?
            .build()
            .map_err(failure)?;
        let update = updater
            .check()
            .await
            .map_err(failure)?
            .ok_or_else(|| failure("no compatible signed release"))?;
        if update.raw_json != authenticated {
            return Err(failure("manifest changed during verification"));
        }
        if update.version != version.to_string() {
            return Err(failure("release manifest version mismatch"));
        }
        // Stream the authenticated artifact ourselves because the updater download
        // callback cannot abort a maliciously oversized response.
        if update.download_url.scheme() != "https" {
            return Err(failure("update URL must use HTTPS"));
        }
        let mut response = client
            .get(update.download_url.clone())
            .send()
            .await
            .map_err(failure)?
            .error_for_status()
            .map_err(failure)?;
        let total = response.content_length();
        const MAX_ARTIFACT: usize = 256 * 1024 * 1024;
        if total.is_some_and(|size| size > MAX_ARTIFACT as u64) {
            return Err(failure("update artifact exceeds 256 MiB"));
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(failure)? {
            if chunk.len() > MAX_ARTIFACT.saturating_sub(bytes.len()) {
                return Err(failure("update artifact exceeds 256 MiB"));
            }
            bytes.extend_from_slice(&chunk);
            let _ = app.emit(
                "desktop-update-progress",
                Progress {
                    downloaded: bytes.len() as u64,
                    total,
                },
            );
        }
        signature::verify(&bytes, &update.signature, key()?).map_err(failure)?;
        Ok(Pending {
            update,
            bytes,
            #[cfg(windows)]
            manifest,
            #[cfg(windows)]
            manifest_signature,
        })
    }
    .await;
    let mut s = state.0.lock();
    s.busy = false;
    match result {
        Ok(pending) => {
            let version = pending.update.version.clone();
            s.pending = Some(pending);
            Ok(Downloaded { version })
        }
        Err(e) => Err(e),
    }
}

#[tauri::command]
pub async fn desktop_update_install_cmd(
    app: tauri::AppHandle,
    updates: State<'_, DesktopUpdater>,
) -> Result<(), String> {
    let pending = {
        let mut s = updates.0.lock();
        if s.busy {
            return Err(failure("another update operation is running"));
        }
        let pending = s
            .pending
            .take()
            .ok_or_else(|| failure("no verified update is ready"))?;
        s.busy = true;
        pending
    };
    let result = install(&app, &pending);
    let mut s = updates.0.lock();
    s.busy = false;
    if result.is_err() {
        s.pending = Some(pending);
    }
    result
}
#[cfg(any(windows, target_os = "macos"))]
fn shutdown(app: &tauri::AppHandle) {
    app.state::<DesktopUpdater>().begin_shutdown();
    let state = app.state::<crate::state::AppState>();
    for session in state.manager.bridge_list() {
        state.automation.cancel_session(&session.id);
    }
    state.manager.shutdown_for_exit();
}
fn install(app: &tauri::AppHandle, pending: &Pending) -> Result<(), String> {
    signature::verify(&pending.bytes, &pending.update.signature, key()?).map_err(failure)?;
    #[cfg(windows)]
    {
        // Staging and permission checks happen before monitoring is interrupted.
        let helper = windows::prepare(
            &pending.bytes,
            &pending.manifest,
            &pending.manifest_signature,
            &pending.update.target,
            &pending.update.version,
        )
        .map_err(failure)?;
        helper.spawn().map_err(failure)?;
        shutdown(app);
        app.exit(0);
        Ok(())
    }
    #[cfg(target_os = "macos")]
    {
        shutdown(app);
        if let Err(error) = pending.update.install(&pending.bytes) {
            app.state::<DesktopUpdater>().reopen_starts();
            return Err(failure(error));
        }
        app.restart();
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        let _ = app;
        Err(failure("desktop updating is unavailable"))
    }
}
pub fn early_entry() -> bool {
    #[cfg(windows)]
    {
        windows::early_entry()
    }
    #[cfg(not(windows))]
    {
        false
    }
}
pub fn acknowledge_ready() {
    #[cfg(windows)]
    {
        windows::acknowledge_ready();
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn downloads_allow_starts_and_failed_install_reopens_them() {
        let updates = DesktopUpdater::default();
        updates.0.lock().busy = true;
        assert!(updates.start_guard().is_ok());
        updates.begin_shutdown();
        assert!(updates.start_guard().unwrap_err().starts_with("desktop_update_failed|"));
        updates.reopen_starts();
        assert!(updates.start_guard().is_ok());
    }

    #[test]
    fn shutdown_waits_for_in_progress_startup() {
        use std::sync::{mpsc, Arc};
        use std::time::Duration;
        let updates = Arc::new(DesktopUpdater::default());
        let permit = updates.start_guard().unwrap();
        let worker_updates = updates.clone();
        let (attempted_tx, attempted_rx) = mpsc::channel();
        let (closed_tx, closed_rx) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            attempted_tx.send(()).unwrap();
            worker_updates.begin_shutdown();
            closed_tx.send(()).unwrap();
        });
        attempted_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(matches!(closed_rx.try_recv(), Err(mpsc::TryRecvError::Empty)));
        drop(permit);
        closed_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        worker.join().unwrap();
        assert!(updates.start_guard().is_err());
    }

    #[test]
    fn concurrent_startup_waits_for_existing_permit() {
        use std::sync::{mpsc, Arc};
        use std::time::Duration;
        let updates = Arc::new(DesktopUpdater::default());
        let permit = updates.start_guard().unwrap();
        let worker_updates = updates.clone();
        let (attempted_tx, attempted_rx) = mpsc::channel();
        let (started_tx, started_rx) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            attempted_tx.send(()).unwrap();
            let _permit = worker_updates.start_guard().unwrap();
            started_tx.send(()).unwrap();
        });
        attempted_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(matches!(started_rx.try_recv(), Err(mpsc::TryRecvError::Empty)));
        drop(permit);
        started_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        worker.join().unwrap();
    }

    #[test]
    fn release_tags_are_canonical_and_stable() {
        assert_eq!(stable_version("v1.2.3").unwrap().to_string(), "1.2.3");
        for tag in [
            "cli-v1.2.3",
            "1.2.3",
            "v01.2.3",
            "v1.2.3-beta",
            "v1.2.3+build",
            "v1.2.3/../x",
        ] {
            assert!(stable_version(tag).is_err(), "{tag}");
        }
    }
}
