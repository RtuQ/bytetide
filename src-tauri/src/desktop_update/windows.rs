use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Child, Command},
    time::{Duration, Instant},
};
use windows_sys::Win32::{
    Foundation::{CloseHandle, WAIT_OBJECT_0},
    System::Threading::{OpenProcess, WaitForSingleObject},
};

const HELPER_FLAG: &str = "--bytetide-update-helper";
const ACK_FLAG: &str = "--bytetide-update-ack";
#[derive(Serialize, Deserialize)]
struct Job {
    parent: u32,
    target: PathBuf,
    platform: String,
    version: String,
}
pub(super) struct Prepared {
    dir: tempfile::TempDir,
    helper: PathBuf,
}
fn sync_write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut file = fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(path)
        .map_err(|e| e.to_string())?;
    file.write_all(bytes)
        .and_then(|_| file.sync_all())
        .map_err(|e| e.to_string())
}
pub(super) fn prepare(
    bytes: &[u8],
    manifest: &[u8],
    signature: &str,
    platform: &str,
    version: &str,
) -> Result<Prepared, String> {
    let target = std::env::current_exe()
        .map_err(|e| e.to_string())?
        .canonicalize()
        .map_err(|e| e.to_string())?;
    // Windows image sections prevent write-open of a running EXE. Check the file
    // attribute and probe its directory by staging there before shutdown.
    if fs::metadata(&target)
        .map_err(|e| e.to_string())?
        .permissions()
        .readonly()
    {
        return Err("installed executable is read-only".into());
    }
    let dir = tempfile::Builder::new()
        .prefix(".bytetide-update-")
        .tempdir_in(target.parent().ok_or("missing executable directory")?)
        .map_err(|e| e.to_string())?;
    sync_write(&dir.path().join("artifact.exe"), bytes)?;
    sync_write(&dir.path().join("manifest.json"), manifest)?;
    sync_write(&dir.path().join("manifest.sig"), signature.as_bytes())?;
    let job = Job {
        parent: std::process::id(),
        target,
        platform: platform.into(),
        version: version.into(),
    };
    sync_write(
        &dir.path().join("job.json"),
        &serde_json::to_vec(&job).map_err(|e| e.to_string())?,
    )?;
    let helper = dir.path().join("helper.exe");
    fs::copy(std::env::current_exe().map_err(|e| e.to_string())?, &helper)
        .map_err(|e| e.to_string())?;
    Ok(Prepared { dir, helper })
}
impl Prepared {
    pub(super) fn spawn(self) -> Result<(), String> {
        let mut child = Command::new(&self.helper)
            .arg(HELPER_FLAG)
            .spawn()
            .map_err(|e| e.to_string())?;
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if self.dir.path().join("helper-ready").exists() {
                // The helper owns cleanup after the parent exits.
                std::mem::forget(self.dir);
                return Ok(());
            }
            if child.try_wait().map_err(|e| e.to_string())?.is_some() {
                return Err("update helper failed to initialize".into());
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        let _ = child.kill();
        let _ = child.wait();
        Err("update helper did not initialize".into())
    }
}
pub(super) fn early_entry() -> bool {
    if !std::env::args_os().any(|a| a == HELPER_FLAG) {
        return false;
    }
    if let Err(error) = helper() {
        eprintln!("desktop update failed: {error}");
    }
    true
}
fn helper() -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let dir = exe.parent().ok_or("missing helper directory")?;
    let job: Job =
        serde_json::from_slice(&fs::read(dir.join("job.json")).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
    if job.target.parent() != dir.parent() {
        return Err("update target must be beside staging directory".into());
    }
    let manifest = fs::read(dir.join("manifest.json")).map_err(|e| e.to_string())?;
    let signature = fs::read_to_string(dir.join("manifest.sig")).map_err(|e| e.to_string())?;
    super::signature::verify(&manifest, signature.trim(), super::key()?)?;
    let manifest: serde_json::Value =
        serde_json::from_slice(&manifest).map_err(|e| e.to_string())?;
    if manifest["version"].as_str() != Some(job.version.as_str()) {
        return Err("manifest version mismatch".into());
    }
    let signature = manifest["platforms"][&job.platform]["signature"]
        .as_str()
        .ok_or("missing platform signature")?;
    let bytes = fs::read(dir.join("artifact.exe")).map_err(|e| e.to_string())?;
    super::signature::verify(&bytes, signature, super::key()?)?;
    // Keep the verified bytes in memory; never install an unverified second read.
    let parent = unsafe { OpenProcess(0x0010_0000, 0, job.parent) };
    if parent.is_null() {
        return Err("cannot open parent process for bounded exit wait".into());
    }
    sync_write(&dir.join("helper-ready"), b"ready")?;
    let waited = unsafe { WaitForSingleObject(parent, 60_000) };
    unsafe {
        CloseHandle(parent);
    }
    if waited != WAIT_OBJECT_0 {
        return Err("parent process did not exit".into());
    }
    let backup = dir.join("backup.exe");
    let replacement = dir.join("replacement.exe");
    sync_write(&replacement, &bytes)?;
    if let Err(error) = super::replacement::replace(&job.target, &replacement, &backup) {
        // A failed staged rename restores the old target; reopen it when possible.
        if job.target.is_file() {
            let _ = Command::new(&job.target).spawn();
        }
        return Err(error);
    }
    let ack = dir.join("app-ready");
    let launched = Command::new(&job.target).arg(ACK_FLAG).arg(&ack).spawn();
    let result = match launched {
        Ok(mut child) => wait_ready(&mut child, &ack),
        Err(e) => Err(e.to_string()),
    };
    if let Err(e) = result {
        super::replacement::rollback(&job.target, &backup)
            .map_err(|rollback| format!("startup: {e}; rollback: {rollback}"))?;
        let _ = Command::new(&job.target).spawn();
        return Err(e);
    }
    let _ = fs::remove_file(&backup);
    // Windows keeps helper.exe locked until this process exits; leave only that file.
    for name in [
        "artifact.exe",
        "replacement.exe",
        "manifest.json",
        "manifest.sig",
        "job.json",
        "helper-ready",
        "app-ready",
    ] {
        let _ = fs::remove_file(dir.join(name));
    }
    Ok(())
}
fn wait_ready(child: &mut Child, ack: &Path) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs(45);
    while Instant::now() < deadline {
        match child.try_wait() {
            Ok(Some(_)) => return Err("updated application exited before ready".into()),
            Ok(None) => {}
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(error.to_string());
            }
        }
        if ack.exists() {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    child.kill().map_err(|e| e.to_string())?;
    child.wait().map_err(|e| e.to_string())?;
    Err("updated application did not become ready".into())
}
pub(super) fn acknowledge_ready() {
    let args: Vec<_> = std::env::args_os().collect();
    if let Some(i) = args.iter().position(|a| a == ACK_FLAG) {
        if let Some(path) = args.get(i + 1) {
            let path = Path::new(path);
            // Only accept the private staging directory next to this executable.
            if let (Some(parent), Ok(exe)) = (path.parent(), std::env::current_exe()) {
                if parent.parent() == exe.parent()
                    && path.file_name().is_some_and(|n| n == "app-ready")
                {
                    if sync_write(path, b"ready").is_ok() {
                        let dir = parent.to_path_buf();
                        // The helper cannot unlink its own mapped image on Windows.
                        // The new process removes it only after the backup is gone.
                        std::thread::spawn(move || {
                            for _ in 0..120 {
                                if !dir.join("backup.exe").exists()
                                    && !dir.join("job.json").exists()
                                {
                                    if fs::remove_file(dir.join("helper.exe")).is_ok() {
                                        let _ = fs::remove_dir(&dir);
                                        return;
                                    }
                                }
                                std::thread::sleep(Duration::from_millis(500));
                            }
                        });
                    }
                }
            }
        }
    }
}
