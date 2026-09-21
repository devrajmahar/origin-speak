//! CLI/bootstrap update client for schema-v2 release manifests.
//!
//! This stages verified manager/runtime payloads. Installing them is a separate
//! bootstrap step because Windows cannot replace the running manager executable
//! and macOS runtime payloads are signed app-bundle ZIPs.

use futures_util::StreamExt;
use reqwest::StatusCode;
use reqwest::header::{CONTENT_RANGE, RANGE};
use semver::Version;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;

const SCHEMA_VERSION: u32 = 2;
const MANIFEST_MAX_BYTES: usize = 128 * 1024;
const PAYLOAD_MAX_BYTES: u64 = 1024 * 1024 * 1024;
const PAYLOAD_DOWNLOAD_ATTEMPTS: usize = 4;
const PAYLOAD_STALL_TIMEOUT: Duration = Duration::from_secs(30);
const PAYLOAD_THROUGHPUT_WINDOW: Duration = Duration::from_secs(15);
const PAYLOAD_MIN_BYTES_PER_SECOND: u64 = 64 * 1024;
pub const DEFAULT_BOOTSTRAP_MANIFEST_URL: &str =
    "https://github.com/devrajmahar/origin-speak/releases/latest/download/bootstrap-update.json";
const GITHUB_RELEASES_BASE_URL: &str =
    "https://github.com/devrajmahar/origin-speak/releases/download/";

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BootstrapManifest {
    pub schema_version: u32,
    pub version: String,
    pub platforms: BTreeMap<String, PlatformPayloads>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlatformPayloads {
    pub manager: Payload,
    pub runtime: Payload,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Payload {
    pub kind: String,
    pub arch: String,
    pub path: String,
    pub url: String,
    pub sha256: String,
}

#[derive(Clone, Debug)]
pub struct AvailableUpdate {
    pub version: Version,
    pub manager: Payload,
    pub runtime: Payload,
}

#[derive(Clone, Debug)]
pub struct StagedUpdate {
    pub version: Version,
    pub manager_path: PathBuf,
    pub manager_sha256: String,
    pub runtime_path: PathBuf,
    pub runtime_sha256: String,
}

pub async fn check_for_update(
    current_version: &str,
    manifest_url: Option<&str>,
) -> Result<Option<AvailableUpdate>, String> {
    let manifest = fetch_manifest(manifest_url).await?;
    select_update(current_version, manifest)
}

pub async fn fetch_release(manifest_url: Option<&str>) -> Result<AvailableUpdate, String> {
    let manifest = fetch_manifest(manifest_url).await?;
    release_for_platform(manifest)
}

pub async fn fetch_release_for_version(version: &Version) -> Result<AvailableUpdate, String> {
    let url = versioned_manifest_url(DEFAULT_BOOTSTRAP_MANIFEST_URL, version)?;
    let release = fetch_release(Some(url.as_str())).await?;
    if release.version != *version {
        return Err(format!(
            "versioned bootstrap manifest returned {}, expected {version}",
            release.version
        ));
    }
    Ok(release)
}

fn versioned_manifest_url(
    latest_manifest_url: &str,
    version: &Version,
) -> Result<reqwest::Url, String> {
    let latest = secure_url(latest_manifest_url, "bootstrap manifest")?;
    if latest.as_str() == DEFAULT_BOOTSTRAP_MANIFEST_URL {
        let versioned = secure_url(
            &format!("{GITHUB_RELEASES_BASE_URL}v{version}/bootstrap-update.json"),
            "versioned bootstrap manifest",
        )?;
        return Ok(versioned);
    }
    let relative = format!("releases/v{version}/bootstrap-update.json");
    let versioned = latest
        .join(&relative)
        .map_err(|error| format!("build versioned bootstrap manifest URL: {error}"))?;
    ensure_secure_url(&versioned, "versioned bootstrap manifest")?;
    Ok(versioned)
}

async fn fetch_manifest(manifest_url: Option<&str>) -> Result<BootstrapManifest, String> {
    let url = secure_url(
        manifest_url.unwrap_or(DEFAULT_BOOTSTRAP_MANIFEST_URL),
        "bootstrap manifest",
    )?;
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(8))
        .timeout(Duration::from_secs(20))
        .user_agent(format!(
            "OriginSpeak/{} cli-manager",
            env!("CARGO_PKG_VERSION")
        ))
        .build()
        .map_err(|error| format!("create update client: {error}"))?;
    let response = client
        .get(url)
        .send()
        .await
        .map_err(|error| format!("fetch bootstrap manifest: {error}"))?;
    ensure_secure_url(response.url(), "bootstrap manifest redirect target")?;
    let response = response
        .error_for_status()
        .map_err(|error| format!("fetch bootstrap manifest: {error}"))?;
    if response
        .content_length()
        .is_some_and(|size| size > MANIFEST_MAX_BYTES as u64)
    {
        return Err("bootstrap manifest exceeds the 128 KiB safety limit".to_string());
    }
    let bytes = response
        .bytes()
        .await
        .map_err(|error| format!("read bootstrap manifest: {error}"))?;
    if bytes.len() > MANIFEST_MAX_BYTES {
        return Err("bootstrap manifest exceeds the 128 KiB safety limit".to_string());
    }
    let manifest: BootstrapManifest = serde_json::from_slice(&bytes)
        .map_err(|error| format!("parse bootstrap manifest: {error}"))?;
    Ok(manifest)
}

pub async fn stage_update<F>(
    update: &AvailableUpdate,
    staging_dir: &Path,
    mut on_progress: F,
) -> Result<StagedUpdate, String>
where
    F: FnMut(&str, u64, Option<u64>),
{
    validate_available_update(update)?;
    prepare_staging_dir(staging_dir, &update.version).await?;
    let result = async {
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(8))
            .timeout(Duration::from_secs(60 * 30))
            .build()
            .map_err(|error| format!("create update client: {error}"))?;
        let manager_path =
            download_payload(&client, &update.manager, staging_dir, &mut on_progress).await?;
        let runtime_path =
            download_payload(&client, &update.runtime, staging_dir, &mut on_progress).await?;
        Ok(StagedUpdate {
            version: update.version.clone(),
            manager_path,
            manager_sha256: update.manager.sha256.clone(),
            runtime_path,
            runtime_sha256: update.runtime.sha256.clone(),
        })
    }
    .await;
    result
}

pub fn verify_staged_payload(
    staging_dir: &Path,
    version: &Version,
    payload: &Path,
    expected_sha256: &str,
) -> Result<(), String> {
    validate_sha256(expected_sha256)?;

    let staging_metadata = fs::symlink_metadata(staging_dir).map_err(|error| {
        format!(
            "inspect update staging directory {} before apply: {error}",
            staging_dir.display()
        )
    })?;
    if !staging_metadata.is_dir() || is_link_or_reparse_point(&staging_metadata) {
        return Err(format!(
            "refusing update staging path that is not a real owned directory: {}",
            staging_dir.display()
        ));
    }

    validate_staging_marker_sync(
        &staging_dir.join(crate::management::UPDATE_STAGING_MARKER_FILE),
        version,
    )?;

    if payload.parent() != Some(staging_dir) {
        return Err(format!(
            "refusing update payload outside the owned staging directory: {}",
            payload.display()
        ));
    }
    verify_regular_file_sha256(payload, expected_sha256, "staged update payload")
}

pub fn verify_regular_file_sha256(
    path: &Path,
    expected_sha256: &str,
    label: &str,
) -> Result<(), String> {
    validate_sha256(expected_sha256)?;
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("inspect {label} {}: {error}", path.display()))?;
    if !metadata.is_file() || is_link_or_reparse_point(&metadata) {
        return Err(format!(
            "refusing {label} that is not a regular file: {}",
            path.display()
        ));
    }
    let actual = sha256_file(path)?;
    if actual != expected_sha256 {
        return Err(format!(
            "SHA-256 verification failed for {label} {}",
            path.display()
        ));
    }
    Ok(())
}

fn validate_staging_marker_sync(marker: &Path, version: &Version) -> Result<(), String> {
    let metadata = fs::symlink_metadata(marker).map_err(|_| {
        format!(
            "refusing unowned update staging directory; marker is missing: {}",
            marker.display()
        )
    })?;
    if !metadata.is_file() || is_link_or_reparse_point(&metadata) {
        return Err(format!(
            "refusing invalid update staging marker: {}",
            marker.display()
        ));
    }
    let contents = fs::read_to_string(marker)
        .map_err(|error| format!("read update staging marker: {error}"))?;
    let expected = crate::management::update_staging_marker_contents(&version.to_string());
    if contents != expected {
        return Err(format!(
            "refusing update staging directory with a mismatched ownership marker: {}",
            marker.display()
        ));
    }
    Ok(())
}

fn sha256_file(path: &Path) -> Result<String, String> {
    let mut file = fs::File::open(path)
        .map_err(|error| format!("open staged payload {}: {error}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 1024 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|error| format!("hash staged payload {}: {error}", path.display()))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn validate_sha256(sha256: &str) -> Result<(), String> {
    if sha256.len() == 64
        && sha256
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        Ok(())
    } else {
        Err("update payload SHA-256 must be 64 lowercase hexadecimal characters".to_string())
    }
}

#[cfg(target_os = "windows")]
fn is_link_or_reparse_point(metadata: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0400;
    metadata.file_type().is_symlink()
        || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(target_os = "windows"))]
fn is_link_or_reparse_point(metadata: &fs::Metadata) -> bool {
    metadata.file_type().is_symlink()
}

pub async fn stage_runtime_payload<F>(
    release: &AvailableUpdate,
    staging_dir: &Path,
    mut on_progress: F,
) -> Result<PathBuf, String>
where
    F: FnMut(&str, u64, Option<u64>),
{
    validate_available_update(release)?;
    prepare_staging_dir(staging_dir, &release.version).await?;
    let result = async {
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(8))
            .timeout(Duration::from_secs(60 * 30))
            .build()
            .map_err(|error| format!("create update client: {error}"))?;
        download_payload(&client, &release.runtime, staging_dir, &mut on_progress).await
    }
    .await;
    result
}

#[cfg(test)]
async fn cleanup_owned_staging_dir(staging_dir: &Path, version: &Version) -> Result<(), String> {
    let metadata = match tokio::fs::symlink_metadata(staging_dir).await {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(format!(
                "inspect update staging directory {} before cleanup: {error}",
                staging_dir.display()
            ));
        }
    };
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(format!(
            "refusing to clean update staging path that is not a real directory: {}",
            staging_dir.display()
        ));
    }
    let marker = staging_dir.join(crate::management::UPDATE_STAGING_MARKER_FILE);
    validate_staging_marker(&marker, version).await?;
    tokio::fs::remove_dir_all(staging_dir)
        .await
        .map_err(|error| {
            format!(
                "remove owned update staging {}: {error}",
                staging_dir.display()
            )
        })
}

async fn prepare_staging_dir(staging_dir: &Path, version: &Version) -> Result<(), String> {
    let marker = staging_dir.join(crate::management::UPDATE_STAGING_MARKER_FILE);
    match tokio::fs::symlink_metadata(staging_dir).await {
        Ok(metadata) => {
            if !metadata.is_dir() || metadata.file_type().is_symlink() {
                return Err(format!(
                    "refusing update staging path that is not an owned directory: {}",
                    staging_dir.display()
                ));
            }
            validate_staging_marker(&marker, version).await?;
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            tokio::fs::create_dir(staging_dir)
                .await
                .map_err(|error| format!("create update staging directory: {error}"))?;
            let contents = crate::management::update_staging_marker_contents(&version.to_string());
            let mut options = tokio::fs::OpenOptions::new();
            options.write(true).create_new(true);
            let claim_result = async {
                let mut file = options
                    .open(&marker)
                    .await
                    .map_err(|error| format!("claim update staging directory: {error}"))?;
                file.write_all(contents.as_bytes())
                    .await
                    .map_err(|error| format!("write update staging marker: {error}"))?;
                file.flush()
                    .await
                    .map_err(|error| format!("flush update staging marker: {error}"))?;
                file.sync_all()
                    .await
                    .map_err(|error| format!("sync update staging marker: {error}"))?;
                Ok::<(), String>(())
            }
            .await;
            if let Err(error) = claim_result {
                let _ = tokio::fs::remove_file(&marker).await;
                let _ = tokio::fs::remove_dir(staging_dir).await;
                return Err(error);
            }
        }
        Err(error) => {
            return Err(format!(
                "inspect update staging directory {}: {error}",
                staging_dir.display()
            ));
        }
    }
    Ok(())
}

async fn validate_staging_marker(marker: &Path, version: &Version) -> Result<(), String> {
    let metadata = tokio::fs::symlink_metadata(marker).await.map_err(|_| {
        format!(
            "refusing to reuse unowned update staging directory; marker is missing: {}",
            marker.display()
        )
    })?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(format!(
            "refusing invalid update staging marker: {}",
            marker.display()
        ));
    }
    let mut file = tokio::fs::File::open(marker)
        .await
        .map_err(|error| format!("open update staging marker: {error}"))?;
    let mut contents = String::new();
    file.read_to_string(&mut contents)
        .await
        .map_err(|error| format!("read update staging marker: {error}"))?;
    let expected = crate::management::update_staging_marker_contents(&version.to_string());
    if contents != expected {
        return Err(format!(
            "refusing update staging directory with a mismatched ownership marker: {}",
            marker.display()
        ));
    }
    Ok(())
}

fn select_update(
    current_version: &str,
    manifest: BootstrapManifest,
) -> Result<Option<AvailableUpdate>, String> {
    if manifest.schema_version != SCHEMA_VERSION {
        return Err(format!(
            "unsupported bootstrap manifest schema {} (expected {SCHEMA_VERSION})",
            manifest.schema_version
        ));
    }
    let current = Version::parse(current_version)
        .map_err(|error| format!("invalid current version {current_version}: {error}"))?;
    let available = Version::parse(&manifest.version)
        .map_err(|error| format!("invalid available version {}: {error}", manifest.version))?;
    if available <= current {
        return Ok(None);
    }
    release_for_platform(manifest).map(Some)
}

fn release_for_platform(manifest: BootstrapManifest) -> Result<AvailableUpdate, String> {
    if manifest.schema_version != SCHEMA_VERSION {
        return Err(format!(
            "unsupported bootstrap manifest schema {} (expected {SCHEMA_VERSION})",
            manifest.schema_version
        ));
    }
    let available = Version::parse(&manifest.version)
        .map_err(|error| format!("invalid available version {}: {error}", manifest.version))?;
    let key = platform_key()?;
    let payloads = manifest
        .platforms
        .get(key)
        .ok_or_else(|| format!("manifest has no payloads for {key}"))?;
    validate_payload_pair(key, payloads)?;
    Ok(AvailableUpdate {
        version: available,
        manager: payloads.manager.clone(),
        runtime: payloads.runtime.clone(),
    })
}

fn validate_payload_pair(platform: &str, payloads: &PlatformPayloads) -> Result<(), String> {
    let (manager_kind, runtime_kind, arch) = match platform {
        "windows-x86_64" => ("cli-manager", "silent-runtime", "x86_64"),
        "macos-universal" => ("cli-manager", "app-bundle-zip", "universal"),
        _ => return Err(format!("unsupported update platform: {platform}")),
    };
    validate_payload(&payloads.manager, manager_kind, arch)?;
    validate_payload(&payloads.runtime, runtime_kind, arch)?;
    Ok(())
}

fn validate_available_update(update: &AvailableUpdate) -> Result<(), String> {
    let platform = platform_key()?;
    let payloads = PlatformPayloads {
        manager: update.manager.clone(),
        runtime: update.runtime.clone(),
    };
    validate_payload_pair(platform, &payloads)
}

fn validate_payload(payload: &Payload, kind: &str, arch: &str) -> Result<(), String> {
    if payload.kind != kind || payload.arch != arch {
        return Err(format!(
            "invalid {} payload kind/architecture",
            payload.kind
        ));
    }
    validate_payload_shape(payload)
}

async fn download_payload<F>(
    client: &reqwest::Client,
    payload: &Payload,
    staging_dir: &Path,
    on_progress: &mut F,
) -> Result<PathBuf, String>
where
    F: FnMut(&str, u64, Option<u64>),
{
    validate_payload_shape(payload)?;
    let url = secure_url(&payload.url, "update payload")?;
    let destination = staging_dir.join(&payload.path);
    let partial = staging_dir.join(format!("{}.part", payload.path));

    if verified_existing_payload(&destination, &payload.sha256).await? {
        let size = tokio::fs::metadata(&destination)
            .await
            .map_err(|error| format!("inspect staged {}: {error}", destination.display()))?
            .len();
        on_progress(&payload.path, size, Some(size));
        return Ok(destination);
    }

    let mut last_error = String::new();
    for attempt in 0..PAYLOAD_DOWNLOAD_ATTEMPTS {
        let reconnect_slow_transfer = attempt + 1 < PAYLOAD_DOWNLOAD_ATTEMPTS;
        match download_payload_attempt(
            client,
            payload,
            &url,
            &partial,
            &destination,
            reconnect_slow_transfer,
            on_progress,
        )
        .await
        {
            Ok(path) => return Ok(path),
            Err(error) => {
                last_error = error;
                if attempt + 1 < PAYLOAD_DOWNLOAD_ATTEMPTS {
                    tokio::time::sleep(Duration::from_secs(1_u64 << attempt)).await;
                }
            }
        }
    }

    Err(format!(
        "download {} failed after {PAYLOAD_DOWNLOAD_ATTEMPTS} attempts: {last_error}",
        payload.path
    ))
}

async fn download_payload_attempt<F>(
    client: &reqwest::Client,
    payload: &Payload,
    url: &reqwest::Url,
    partial: &Path,
    destination: &Path,
    reconnect_slow_transfer: bool,
    on_progress: &mut F,
) -> Result<PathBuf, String>
where
    F: FnMut(&str, u64, Option<u64>),
{
    let mut resume_offset = safe_partial_len(partial).await?;
    if resume_offset > PAYLOAD_MAX_BYTES {
        tokio::fs::remove_file(partial)
            .await
            .map_err(|error| format!("remove oversized partial {}: {error}", partial.display()))?;
        resume_offset = 0;
    }

    let mut request = client.get(url.clone());
    if resume_offset > 0 {
        request = request.header(RANGE, format!("bytes={resume_offset}-"));
    }
    let response = request
        .send()
        .await
        .map_err(|error| format!("download {}: {error}", payload.path))?;
    ensure_secure_url(response.url(), "update payload redirect target")?;

    if resume_offset > 0 && response.status() == StatusCode::RANGE_NOT_SATISFIABLE {
        if hash_file_async(partial.to_path_buf()).await? == payload.sha256 {
            return install_verified_partial(partial, destination, payload).await;
        }
        tokio::fs::remove_file(partial)
            .await
            .map_err(|error| format!("remove rejected partial {}: {error}", partial.display()))?;
        return Err(format!(
            "server rejected the saved range for {}; restarting",
            payload.path
        ));
    }

    let status = response.status();
    let response = response
        .error_for_status()
        .map_err(|error| format!("download {}: {error}", payload.path))?;
    let content_length = response.content_length();
    let content_range = response
        .headers()
        .get(CONTENT_RANGE)
        .and_then(|value| value.to_str().ok());

    let (append, expected_total) = match status {
        StatusCode::PARTIAL_CONTENT => {
            let total = parse_content_range(content_range, resume_offset, content_length)?;
            (resume_offset > 0, Some(total))
        }
        StatusCode::OK => {
            if resume_offset > 0 {
                tokio::fs::remove_file(partial).await.map_err(|error| {
                    format!("restart partial download {}: {error}", partial.display())
                })?;
                resume_offset = 0;
            }
            (false, content_length)
        }
        _ => {
            return Err(format!(
                "download {} returned unexpected HTTP {status}",
                payload.path
            ));
        }
    };

    if expected_total.is_some_and(|size| size > PAYLOAD_MAX_BYTES) {
        return Err(format!("{} exceeds the 1 GiB safety limit", payload.path));
    }

    let mut file = open_safe_partial(partial, append).await?;
    let mut stream = response.bytes_stream();
    let mut downloaded = resume_offset;
    let mut throughput_started = Instant::now();
    let mut throughput_bytes = downloaded;
    on_progress(&payload.path, downloaded, expected_total);
    loop {
        let next = tokio::time::timeout(PAYLOAD_STALL_TIMEOUT, stream.next())
            .await
            .map_err(|_| {
                format!(
                    "download {} stalled for {} seconds",
                    payload.path,
                    PAYLOAD_STALL_TIMEOUT.as_secs()
                )
            })?;
        let Some(chunk) = next else { break };
        let chunk = chunk.map_err(|error| format!("download {}: {error}", payload.path))?;
        downloaded = downloaded.saturating_add(chunk.len() as u64);
        if downloaded > PAYLOAD_MAX_BYTES
            || expected_total.is_some_and(|expected| downloaded > expected)
        {
            return Err(format!("{} exceeded its declared size", payload.path));
        }
        file.write_all(&chunk)
            .await
            .map_err(|error| format!("write {}: {error}", partial.display()))?;
        on_progress(&payload.path, downloaded, expected_total);

        let throughput_elapsed = throughput_started.elapsed();
        if throughput_elapsed >= PAYLOAD_THROUGHPUT_WINDOW {
            let window_bytes = downloaded.saturating_sub(throughput_bytes);
            if reconnect_slow_transfer && transfer_is_too_slow(window_bytes, throughput_elapsed) {
                file.flush()
                    .await
                    .map_err(|error| format!("flush {}: {error}", partial.display()))?;
                return Err(format!(
                    "download {} stayed below {} KiB/s for {} seconds; reconnecting",
                    payload.path,
                    PAYLOAD_MIN_BYTES_PER_SECOND / 1024,
                    throughput_elapsed.as_secs()
                ));
            }
            throughput_started = Instant::now();
            throughput_bytes = downloaded;
        }
    }
    file.flush()
        .await
        .map_err(|error| format!("flush {}: {error}", partial.display()))?;
    file.sync_all()
        .await
        .map_err(|error| format!("sync {}: {error}", partial.display()))?;
    drop(file);

    if downloaded == 0 {
        return Err(format!("downloaded {} is empty", payload.path));
    }
    if expected_total.is_some_and(|expected| downloaded != expected) {
        return Err(format!(
            "download {} ended at {downloaded} bytes, expected {}",
            payload.path,
            expected_total.unwrap_or_default()
        ));
    }
    if hash_file_async(partial.to_path_buf()).await? != payload.sha256 {
        tokio::fs::remove_file(partial)
            .await
            .map_err(|error| format!("remove corrupt partial {}: {error}", partial.display()))?;
        return Err(format!("SHA-256 verification failed for {}", payload.path));
    }

    install_verified_partial(partial, destination, payload).await
}

fn transfer_is_too_slow(bytes: u64, elapsed: Duration) -> bool {
    elapsed >= PAYLOAD_THROUGHPUT_WINDOW
        && (bytes as u128) * 1_000_000_000
            < (PAYLOAD_MIN_BYTES_PER_SECOND as u128) * elapsed.as_nanos()
}

async fn verified_existing_payload(path: &Path, expected_sha256: &str) -> Result<bool, String> {
    let metadata = match tokio::fs::symlink_metadata(path).await {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => {
            return Err(format!(
                "inspect staged payload {}: {error}",
                path.display()
            ));
        }
    };
    if !metadata.is_file() || is_link_or_reparse_point(&metadata) {
        return Err(format!(
            "refusing staged payload that is not a regular file: {}",
            path.display()
        ));
    }
    if metadata.len() == 0 || metadata.len() > PAYLOAD_MAX_BYTES {
        tokio::fs::remove_file(path).await.map_err(|error| {
            format!("remove invalid staged payload {}: {error}", path.display())
        })?;
        return Ok(false);
    }
    if hash_file_async(path.to_path_buf()).await? == expected_sha256 {
        Ok(true)
    } else {
        tokio::fs::remove_file(path).await.map_err(|error| {
            format!("remove corrupt staged payload {}: {error}", path.display())
        })?;
        Ok(false)
    }
}

async fn install_verified_partial(
    partial: &Path,
    destination: &Path,
    payload: &Payload,
) -> Result<PathBuf, String> {
    match tokio::fs::remove_file(&destination).await {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(format!(
                "remove stale staged payload {}: {error}",
                destination.display()
            ));
        }
    }
    if let Err(error) = tokio::fs::rename(&partial, &destination).await {
        return Err(format!("stage {}: {error}", payload.path));
    }
    Ok(destination.to_path_buf())
}

async fn safe_partial_len(path: &Path) -> Result<u64, String> {
    match tokio::fs::symlink_metadata(path).await {
        Ok(metadata) => {
            if !metadata.is_file() || is_link_or_reparse_point(&metadata) {
                return Err(format!(
                    "refusing non-regular update partial: {}",
                    path.display()
                ));
            }
            Ok(metadata.len())
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(0),
        Err(error) => Err(format!(
            "inspect update partial {}: {error}",
            path.display()
        )),
    }
}

async fn open_safe_partial(path: &Path, append: bool) -> Result<tokio::fs::File, String> {
    let existing = safe_partial_len(path).await?;
    let mut options = tokio::fs::OpenOptions::new();
    if append {
        if existing == 0 {
            return Err(format!(
                "resume partial is missing or empty: {}",
                path.display()
            ));
        }
        options.append(true);
    } else {
        match tokio::fs::remove_file(path).await {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(format!("remove stale partial {}: {error}", path.display()));
            }
        }
        options.write(true).create_new(true);
    }
    options
        .open(path)
        .await
        .map_err(|error| format!("open {}: {error}", path.display()))
}

async fn hash_file_async(path: PathBuf) -> Result<String, String> {
    tokio::task::spawn_blocking(move || sha256_file(&path))
        .await
        .map_err(|error| format!("hash update payload task failed: {error}"))?
}

fn parse_content_range(
    content_range: Option<&str>,
    expected_start: u64,
    content_length: Option<u64>,
) -> Result<u64, String> {
    let value = content_range.ok_or_else(|| "resume response omitted Content-Range".to_string())?;
    let value = value
        .strip_prefix("bytes ")
        .ok_or_else(|| format!("invalid Content-Range: {value}"))?;
    let (range, total) = value
        .split_once('/')
        .ok_or_else(|| format!("invalid Content-Range: {value}"))?;
    let (start, end) = range
        .split_once('-')
        .ok_or_else(|| format!("invalid Content-Range: {value}"))?;
    let start = start
        .parse::<u64>()
        .map_err(|_| format!("invalid Content-Range start: {value}"))?;
    let end = end
        .parse::<u64>()
        .map_err(|_| format!("invalid Content-Range end: {value}"))?;
    let total = total
        .parse::<u64>()
        .map_err(|_| format!("invalid Content-Range total: {value}"))?;
    if start != expected_start || end < start || total <= end {
        return Err(format!("unexpected Content-Range: {value}"));
    }
    let range_len = end - start + 1;
    if content_length.is_some_and(|length| length != range_len) {
        return Err(format!("Content-Range length mismatch: {value}"));
    }
    Ok(total)
}

fn validate_payload_shape(payload: &Payload) -> Result<(), String> {
    if payload.path.is_empty()
        || payload.path.contains('/')
        || payload.path.contains('\\')
        || Path::new(&payload.path)
            .file_name()
            .and_then(|name| name.to_str())
            != Some(payload.path.as_str())
    {
        return Err("update payload path must be a single filename".to_string());
    }
    if payload.sha256.len() != 64
        || !payload
            .sha256
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err(
            "update payload SHA-256 must be 64 lowercase hexadecimal characters".to_string(),
        );
    }
    secure_url(&payload.url, "update payload")?;
    Ok(())
}

fn secure_url(raw: &str, label: &str) -> Result<reqwest::Url, String> {
    let url = reqwest::Url::parse(raw).map_err(|error| format!("invalid {label} URL: {error}"))?;
    ensure_secure_url(&url, label)?;
    Ok(url)
}

fn ensure_secure_url(url: &reqwest::Url, label: &str) -> Result<(), String> {
    if url.scheme() != "https" || !url.username().is_empty() || url.password().is_some() {
        return Err(format!("{label} URL must be credential-free HTTPS"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn payload(kind: &str, arch: &str, path: &str) -> Payload {
        Payload {
            kind: kind.to_string(),
            arch: arch.to_string(),
            path: path.to_string(),
            url: format!("https://example.invalid/{path}"),
            sha256: "a".repeat(64),
        }
    }

    #[test]
    fn payload_validation_rejects_path_traversal_and_bad_hashes() {
        let mut candidate = payload("cli-manager", "x86_64", "../origin.exe");
        assert!(validate_payload_shape(&candidate).is_err());
        candidate.path = "origin.exe".to_string();
        candidate.sha256 = "ABC".to_string();
        assert!(validate_payload_shape(&candidate).is_err());
    }

    #[test]
    fn redirect_targets_must_remain_credential_free_https() {
        let https = reqwest::Url::parse("https://updates.example/path").unwrap();
        assert!(ensure_secure_url(&https, "redirect").is_ok());
        let http = reqwest::Url::parse("http://updates.example/path").unwrap();
        assert!(ensure_secure_url(&http, "redirect").is_err());
        let credentialed = reqwest::Url::parse("https://user@example.com/path").unwrap();
        assert!(ensure_secure_url(&credentialed, "redirect").is_err());
    }

    #[test]
    fn resumed_download_requires_an_exact_content_range() {
        assert_eq!(
            parse_content_range(Some("bytes 1024-2047/4096"), 1024, Some(1024)).unwrap(),
            4096
        );
        assert!(parse_content_range(Some("bytes 0-1023/4096"), 1024, Some(1024)).is_err());
        assert!(parse_content_range(Some("bytes 1024-2047/4096"), 1024, Some(512)).is_err());
        assert!(parse_content_range(None, 1024, Some(1024)).is_err());
    }

    #[test]
    fn slow_transfer_reconnect_uses_a_sustained_conservative_floor() {
        let window = PAYLOAD_THROUGHPUT_WINDOW;
        let floor_bytes = PAYLOAD_MIN_BYTES_PER_SECOND * window.as_secs();

        assert!(!transfer_is_too_slow(
            PAYLOAD_MIN_BYTES_PER_SECOND,
            window - Duration::from_secs(1)
        ));
        assert!(transfer_is_too_slow(floor_bytes - 1, window));
        assert!(!transfer_is_too_slow(floor_bytes, window));
        assert!(!transfer_is_too_slow(floor_bytes * 10, window));
    }

    #[tokio::test]
    async fn regular_partial_is_retained_for_resume() {
        let root = std::env::temp_dir().join(format!(
            "origin-speak-update-resume-test-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let partial = root.join("payload.part");
        std::fs::write(&partial, b"saved bytes").unwrap();
        assert_eq!(safe_partial_len(&partial).await.unwrap(), 11);
        let file = open_safe_partial(&partial, true).await.unwrap();
        drop(file);
        assert_eq!(std::fs::read(&partial).unwrap(), b"saved bytes");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn default_manifest_uses_github_releases_latest_download() {
        assert_eq!(
            DEFAULT_BOOTSTRAP_MANIFEST_URL,
            "https://github.com/devrajmahar/origin-speak/releases/latest/download/bootstrap-update.json"
        );
    }

    #[test]
    fn github_versioned_manifest_is_pinned_to_exact_release_download() {
        let version = Version::parse("1.2.3").unwrap();
        let url = versioned_manifest_url(DEFAULT_BOOTSTRAP_MANIFEST_URL, &version).unwrap();
        assert_eq!(
            url.as_str(),
            "https://github.com/devrajmahar/origin-speak/releases/download/v1.2.3/bootstrap-update.json"
        );
    }

    #[test]
    fn custom_non_github_manifest_keeps_relative_versioned_release_path() {
        let version = Version::parse("1.2.3").unwrap();
        let url = versioned_manifest_url(
            "https://updates.example.invalid/channel/bootstrap-update.json",
            &version,
        )
        .unwrap();
        assert_eq!(
            url.as_str(),
            "https://updates.example.invalid/channel/releases/v1.2.3/bootstrap-update.json"
        );
    }

    #[tokio::test]
    async fn update_partial_refuses_symlink() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            let root = std::env::temp_dir().join(format!(
                "origin-speak-update-partial-test-{}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(&root).unwrap();
            let target = root.join("target");
            let partial = root.join("payload.part");
            std::fs::write(&target, b"keep").unwrap();
            symlink(&target, &partial).unwrap();
            assert!(safe_partial_len(&partial).await.is_err());
            assert_eq!(std::fs::read(&target).unwrap(), b"keep");
            std::fs::remove_dir_all(&root).unwrap();
        }
    }

    #[tokio::test]
    async fn owned_staging_cleanup_requires_exact_marker() {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "origin-speak-update-cleanup-test-{}-{nonce}",
            std::process::id()
        ));
        let version = Version::parse("1.2.3").unwrap();
        let owned = root.join("owned");
        let unowned = root.join("unowned");
        std::fs::create_dir_all(owned.join("nested")).unwrap();
        std::fs::create_dir_all(&unowned).unwrap();
        std::fs::write(
            owned.join(crate::management::UPDATE_STAGING_MARKER_FILE),
            crate::management::update_staging_marker_contents("1.2.3"),
        )
        .unwrap();
        std::fs::write(owned.join("nested/payload.part"), b"partial").unwrap();
        std::fs::write(unowned.join("payload.part"), b"keep").unwrap();

        cleanup_owned_staging_dir(&owned, &version).await.unwrap();
        assert!(!owned.exists());
        assert!(cleanup_owned_staging_dir(&unowned, &version).await.is_err());
        assert!(unowned.join("payload.part").is_file());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn staged_payload_revalidation_requires_hash_and_exact_ownership_marker() {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "origin-speak-update-revalidate-test-{}-{nonce}",
            std::process::id()
        ));
        let version = Version::parse("2.3.4").unwrap();
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join(crate::management::UPDATE_STAGING_MARKER_FILE),
            crate::management::update_staging_marker_contents("2.3.4"),
        )
        .unwrap();
        let staged = root.join("origin.exe");
        std::fs::write(&staged, b"verified manager").unwrap();
        let expected = sha256_file(&staged).unwrap();

        verify_staged_payload(&root, &version, &staged, &expected).unwrap();

        std::fs::write(&staged, b"tampered manager").unwrap();
        assert!(verify_staged_payload(&root, &version, &staged, &expected).is_err());

        std::fs::write(&staged, b"verified manager").unwrap();
        std::fs::write(
            root.join(crate::management::UPDATE_STAGING_MARKER_FILE),
            crate::management::update_staging_marker_contents("9.9.9"),
        )
        .unwrap();
        assert!(verify_staged_payload(&root, &version, &staged, &expected).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn installed_payload_hash_verification_detects_post_copy_tampering() {
        let root = std::env::temp_dir().join(format!(
            "origin-speak-update-installed-hash-test-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let installed = root.join("origin-runtime.exe");
        std::fs::write(&installed, b"expected runtime").unwrap();
        let expected = sha256_file(&installed).unwrap();
        verify_regular_file_sha256(&installed, &expected, "installed runtime").unwrap();
        std::fs::write(&installed, b"changed runtime").unwrap();
        assert!(verify_regular_file_sha256(&installed, &expected, "installed runtime").is_err());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn update_payload_hashing_fits_on_a_small_thread_stack() {
        let root = std::env::temp_dir().join(format!(
            "origin-speak-update-small-stack-hash-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let payload = root.join("payload.bin");
        std::fs::write(&payload, b"abc").unwrap();
        let thread_payload = payload.clone();
        let digest = std::thread::Builder::new()
            .stack_size(128 * 1024)
            .spawn(move || sha256_file(&thread_payload))
            .expect("spawn small-stack update hashing thread")
            .join()
            .expect("small-stack update hashing thread panicked")
            .expect("hash update payload");
        assert_eq!(
            digest,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}

fn platform_key() -> Result<&'static str, String> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("windows", "x86_64") => Ok("windows-x86_64"),
        ("macos", "x86_64" | "aarch64") => Ok("macos-universal"),
        (os, arch) => Err(format!("updates are not published for {os}/{arch}")),
    }
}
