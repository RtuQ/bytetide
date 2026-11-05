//! 显式 CLI 更新：发布选择、校验及同目录原子替换，不在启动时调用。

use std::io::Read;
#[cfg(unix)]
use std::io::Write;
use std::path::Path;
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use reqwest::blocking::Client;
use semver::Version;
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::args::UpdateArgs;

const API: &str = "https://api.github.com/repos/RtuQ/bytetide/releases";
const MAX_ARCHIVE: u64 = 128 * 1024 * 1024;
const MAX_BINARY: u64 = 256 * 1024 * 1024;

#[derive(Debug, Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
}

#[derive(Debug, Deserialize)]
struct Release {
    tag_name: String,
    draft: bool,
    prerelease: bool,
    assets: Vec<Asset>,
}

/// 将发布标签解析为稳定语义版本，忽略不属于 CLI/桌面命名空间的标签。
fn release_version(tag: &str) -> Option<Version> {
    let version = Version::parse(
        tag.strip_prefix("cli-v")
            .or_else(|| tag.strip_prefix('v'))?,
    )
    .ok()?;
    (version.pre.is_empty() && version.build.is_empty()).then_some(version)
}

/// 按版本排序选择完整平台产物；同版本优先独立 CLI 发布。
fn select_release<'a>(
    releases: &'a [Release],
    archive: &str,
    pinned: Option<&Version>,
) -> Option<(&'a Release, Version)> {
    releases
        .iter()
        .filter(|release| !release.draft && !release.prerelease)
        .filter_map(|release| {
            let version = release_version(&release.tag_name)?;
            let has = |name: &str| release.assets.iter().any(|asset| asset.name == name);
            (pinned.is_none_or(|pin| *pin == version)
                && has(archive)
                && has(&format!("{archive}.sha256")))
            .then_some((release, version))
        })
        .max_by(|(a, av), (b, bv)| {
            av.cmp(bv).then_with(|| {
                a.tag_name
                    .starts_with("cli-v")
                    .cmp(&b.tag_name.starts_with("cli-v"))
            })
        })
}

/// 将编译平台映射到现有发布产物；GNU Linux 也使用静态 musl 包。
fn target() -> Result<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") => Ok("x86_64-unknown-linux-musl"),
        ("linux", "aarch64") => Ok("aarch64-unknown-linux-musl"),
        ("macos", "aarch64") => Ok("aarch64-apple-darwin"),
        _ => bail!("此平台暂不支持 CLI 自动更新，请手动下载安装"),
    }
}

/// HTTPS 下载带总超时和大小上限；禁止通过重定向降级到 HTTP。
fn download(client: &Client, url: &str, limit: u64) -> Result<Vec<u8>> {
    if !url.starts_with("https://") {
        bail!("拒绝非 HTTPS 更新地址");
    }
    let response = client.get(url).send()?.error_for_status()?;
    if response.content_length().is_some_and(|size| size > limit) {
        bail!("更新响应超过大小限制");
    }
    let mut bytes = Vec::new();
    response.take(limit + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        bail!("更新响应超过大小限制");
    }
    Ok(bytes)
}

/// 校验 sidecar 中的文件名和 SHA-256，拒绝缺失、歧义和错误校验值。
fn verify_checksum(bytes: &[u8], sidecar: &[u8], name: &str) -> Result<()> {
    let text = std::str::from_utf8(sidecar)?;
    let fields: Vec<_> = text.split_whitespace().collect();
    if fields.len() != 2 || fields[1].trim_start_matches('*') != name {
        bail!("校验文件格式或文件名错误");
    }
    let expected = fields[0];
    if expected.len() != 64 || !expected.bytes().all(|b| b.is_ascii_hexdigit()) {
        bail!("无效 SHA-256 校验值");
    }
    if format!("{:x}", Sha256::digest(bytes)) != expected.to_ascii_lowercase() {
        bail!("SHA-256 校验失败，原程序未修改");
    }
    Ok(())
}

/// 只读取根目录下的常规 bytetide 文件，不解包路径、链接或其他文件。
fn extract_binary(archive: &[u8]) -> Result<Vec<u8>> {
    let limit = MAX_BINARY + 1024 * 1024;
    let decoder = flate2::read::GzDecoder::new(archive);
    let mut unpacked = Vec::new();
    decoder.take(limit + 1).read_to_end(&mut unpacked)?;
    if unpacked.len() as u64 > limit {
        bail!("解压后的更新包超过大小限制");
    }
    let mut tar = tar::Archive::new(std::io::Cursor::new(unpacked));
    let mut binary = None;
    for entry in tar.entries()? {
        let mut entry = entry?;
        if entry.path()?.as_ref() != Path::new("bytetide") {
            continue;
        }
        if binary.is_some() || !entry.header().entry_type().is_file() || entry.size() > MAX_BINARY {
            bail!("更新包内的 bytetide 文件无效");
        }
        let mut bytes = Vec::new();
        entry
            .by_ref()
            .take(MAX_BINARY + 1)
            .read_to_end(&mut bytes)?;
        if bytes.is_empty() || bytes.len() as u64 > MAX_BINARY {
            bail!("更新二进制为空或超过大小限制");
        }
        binary = Some(bytes);
    }
    binary.ok_or_else(|| anyhow!("更新包缺少 bytetide"))
}

/// 同目录暂存、验证版本后原子替换；失败自动清理临时文件，保留旧程序。
#[cfg(unix)]
fn install(binary: &[u8], destination: &Path, version: &Version) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let destination = destination.canonicalize()?;
    let parent = destination.parent().context("安装目录无效")?;
    let mode = std::fs::metadata(&destination)?.permissions().mode() & 0o777;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)
        .context("安装目录不可写，请以拥有该目录写权限的用户更新")?;
    temporary.write_all(binary)?;
    temporary
        .as_file()
        .set_permissions(std::fs::Permissions::from_mode(mode))?;
    temporary.as_file().sync_all()?;
    // macOS/Linux 执行验证前先关闭写句柄，避免 ETXTBSY。
    let temporary = temporary.into_temp_path();
    let mut child = std::process::Command::new(&temporary)
        .arg("--version")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()?;
    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    while child.try_wait()?.is_none() {
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            bail!("新程序版本验证超时，原程序未修改");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let output = child.wait_with_output()?;
    if !output.status.success()
        || String::from_utf8_lossy(&output.stdout).trim() != format!("bytetide {version}")
    {
        bail!("新程序版本验证失败，原程序未修改");
    }
    temporary
        .persist(&destination)
        .map_err(|error| anyhow!("替换失败: {error}"))?;
    Ok(())
}

#[cfg(not(unix))]
fn install(_binary: &[u8], _destination: &Path, _version: &Version) -> Result<()> {
    bail!("此平台暂不支持 CLI 自动更新")
}

/// 更新命令的唯一联网入口；未显式指定版本时不降级。
fn run(args: &UpdateArgs) -> Result<()> {
    let archive = format!("bytetide-{}.tar.gz", target()?);
    let pinned = args
        .version
        .as_deref()
        .map(|text| -> Result<Version> {
            let version = Version::parse(text.strip_prefix('v').unwrap_or(text))?;
            if !version.pre.is_empty() || !version.build.is_empty() {
                bail!("仅支持稳定版本");
            }
            Ok(version)
        })
        .transpose()?;
    let client = Client::builder()
        .user_agent(concat!("bytetide/", env!("CARGO_PKG_VERSION")))
        .https_only(true)
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(180))
        .build()?;
    let mut releases = Vec::new();
    for page in 1.. {
        let bytes = download(
            &client,
            &format!("{API}?per_page=100&page={page}"),
            16 * 1024 * 1024,
        )?;
        let batch: Vec<Release> = serde_json::from_slice(&bytes)?;
        let done = batch.len() < 100;
        releases.extend(batch);
        if done {
            break;
        }
        if page >= 100 {
            bail!("发布列表超过分页限制，请指定手动下载");
        }
    }
    let (release, version) = select_release(&releases, &archive, pinned.as_ref())
        .context("没有包含当前平台程序及 SHA-256 校验文件的正式发布")?;
    let current = Version::parse(env!("CARGO_PKG_VERSION"))?;
    if version == current || (pinned.is_none() && version < current) {
        eprintln!("当前已是最新版或指定版本：{current}");
        return Ok(());
    }
    eprintln!("ByteTide CLI：{current} → {version}");
    if args.check {
        return Ok(());
    }
    let asset_url = |name: &str| -> Result<&str> {
        release
            .assets
            .iter()
            .find(|asset| asset.name == name)
            .map(|asset| asset.browser_download_url.as_str())
            .context("发布产物缺失")
    };
    let checksum = download(&client, asset_url(&format!("{archive}.sha256"))?, 4096)?;
    eprintln!("正在下载 {}…", release.tag_name);
    let bytes = download(&client, asset_url(&archive)?, MAX_ARCHIVE)?;
    verify_checksum(&bytes, &checksum, &archive)?;
    let binary = extract_binary(&bytes)?;
    install(&binary, &std::env::current_exe()?, &version)?;
    eprintln!("已更新至 {version}，下次运行生效");
    Ok(())
}

/// 错误仅写 stderr；更新失败返回非零退出码。
pub fn cmd_update(args: &UpdateArgs) -> i32 {
    match run(args) {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("更新失败：{error:#}");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn release(tag: &str, complete: bool) -> Release {
        Release {
            tag_name: tag.into(),
            draft: false,
            prerelease: false,
            assets: if complete {
                vec![
                    Asset {
                        name: "test.tar.gz".into(),
                        browser_download_url: "https://example.com/a".into(),
                    },
                    Asset {
                        name: "test.tar.gz.sha256".into(),
                        browser_download_url: "https://example.com/b".into(),
                    },
                ]
            } else {
                vec![]
            },
        }
    }

    #[test]
    fn selection_uses_semver_and_complete_assets() {
        let releases = vec![
            release("v0.10.0", true),
            release("cli-v0.9.0", true),
            release("v0.11.0", false),
        ];
        assert_eq!(
            select_release(&releases, "test.tar.gz", None).unwrap().1,
            Version::new(0, 10, 0)
        );
        assert!(select_release(&releases, "other.tar.gz", None).is_none());
    }

    #[test]
    fn selection_prefers_cli_and_excludes_unstable_releases() {
        let mut draft = release("v9.0.0", true);
        draft.draft = true;
        let releases = vec![
            draft,
            release("v1.0.0", true),
            release("cli-v1.0.0", true),
            release("v2.0.0-rc.1", true),
        ];
        assert_eq!(
            select_release(&releases, "test.tar.gz", None)
                .unwrap()
                .0
                .tag_name,
            "cli-v1.0.0"
        );
        assert!(select_release(&releases, "test.tar.gz", Some(&Version::new(0, 1, 0))).is_none());
    }

    #[test]
    fn checksum_rejects_corruption_and_wrong_name() {
        let sidecar = format!("{:x}  test.tar.gz\n", Sha256::digest(b"valid"));
        assert!(verify_checksum(b"valid", sidecar.as_bytes(), "test.tar.gz").is_ok());
        assert!(verify_checksum(b"changed", sidecar.as_bytes(), "test.tar.gz").is_err());
        assert!(verify_checksum(b"valid", sidecar.as_bytes(), "other.tar.gz").is_err());
    }

    #[test]
    fn archive_requires_regular_root_binary() {
        let encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        let mut tar = tar::Builder::new(encoder);
        let mut header = tar::Header::new_gnu();
        header.set_size(4);
        header.set_mode(0o755);
        header.set_cksum();
        tar.append_data(&mut header, "bytetide", &b"test"[..])
            .unwrap();
        let bytes = tar.into_inner().unwrap().finish().unwrap();
        assert_eq!(extract_binary(&bytes).unwrap(), b"test");
        assert!(extract_binary(b"corrupt").is_err());
        let mut corrupt = bytes;
        let trailer = corrupt.len() - 8;
        corrupt[trailer..].fill(b'X');
        assert!(extract_binary(&corrupt).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn invalid_binary_preserves_old_installation() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bytetide");
        std::fs::write(&path, b"old binary").unwrap();
        assert!(install(b"invalid executable", &path, &Version::new(1, 0, 0)).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"old binary");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn replacement_preserves_symlink_and_rejects_wrong_version() {
        use std::os::unix::fs::{symlink, PermissionsExt};
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("real-bytetide");
        let link = dir.path().join("bytetide");
        std::fs::write(&target, b"old binary").unwrap();
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o755)).unwrap();
        symlink(&target, &link).unwrap();
        let binary = b"#!/bin/sh\necho 'bytetide 1.0.0'\n";
        assert!(install(binary, &link, &Version::new(2, 0, 0)).is_err());
        assert_eq!(std::fs::read(&target).unwrap(), b"old binary");
        install(binary, &link, &Version::new(1, 0, 0)).unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), binary);
        assert!(std::fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink());
    }
}
