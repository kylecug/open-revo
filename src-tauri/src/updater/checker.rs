// OpenRevo - Auto Updater Checker & Dual-Source Race Engine
// Pure ASCII compliant

use std::time::Duration;
use semver::Version;
use super::types::{ReleaseManifest, UpdateCheckResult};

// Forks should set these at build time so their builds do not self-update from upstream.
pub const DEFAULT_GITHUB_URL: &str = match option_env!("OPENREVO_UPDATE_GITHUB_URL") {
    Some(url) => url,
    None => "https://raw.githubusercontent.com/faintonce/open-revo/main/latest.json",
};
pub const DEFAULT_GITEE_URL: &str = match option_env!("OPENREVO_UPDATE_GITEE_URL") {
    Some(url) => url,
    None => "https://gitee.com/faintonce/open-revo/raw/master/latest.json",
};

/// The manifest override runs inside an elevated process, so release builds must ignore it.
fn debug_update_url_override() -> Option<String> {
    if cfg!(debug_assertions) {
        std::env::var("OPENREVO_UPDATE_URL").ok()
    } else {
        None
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DistributionType {
    Installer,
    Portable,
}

/// Detects whether current running binary is an installed copy or portable copy
pub fn detect_distribution_type() -> DistributionType {
    if let Ok(exe_path) = std::env::current_exe() {
        if let Some(parent) = exe_path.parent() {
            if parent.join("uninstall.exe").exists() || parent.join("uninst.exe").exists() {
                return DistributionType::Installer;
            }
            let path_str = parent.to_string_lossy().to_lowercase();
            if path_str.contains("program files") {
                return DistributionType::Installer;
            }
        }
    }
    DistributionType::Portable
}

/// Creates a standard ureq agent configured with timeouts and native TLS
pub fn create_http_agent(timeout_ms: u64) -> ureq::Agent {
    let mut builder = ureq::builder()
        .timeout_connect(Duration::from_millis(timeout_ms))
        .timeout_read(Duration::from_millis(timeout_ms))
        .user_agent("OpenRevo-Updater");

    match ureq::native_tls::TlsConnector::new() {
        Ok(tls) => {
            builder = builder.tls_connector(std::sync::Arc::new(tls));
        }
        Err(e) => {
            crate::log_error!("UPDATER", "Failed to initialize native TLS connector: {}", e);
        }
    }

    builder.build()
}

/// Single manifest fetch helper with timeout and branch fallback (master <-> main)
fn fetch_single_manifest(url: &str, timeout_ms: u64) -> Result<ReleaseManifest, String> {
    let agent = create_http_agent(timeout_ms);

    let try_fetch = |target: &str| -> Result<Option<ReleaseManifest>, String> {
        let resp = match agent.get(target).call() {
            Ok(r) => r,
            Err(ureq::Error::Status(404, _)) => return Ok(None),
            Err(e) => return Err(format!("Network request failed: {}", e)),
        };

        if resp.status() == 404 {
            return Ok(None);
        }

        if resp.status() != 200 {
            return Err(format!("Server returned HTTP {}", resp.status()));
        }

        resp.into_json::<ReleaseManifest>()
            .map(Some)
            .map_err(|e| format!("Failed to parse release manifest JSON: {}", e))
    };

    // 1. Primary attempt
    if let Some(manifest) = try_fetch(url)? {
        return Ok(manifest);
    }

    // 2. Branch fallback: master <-> main
    let alt_url = if url.contains("/raw/master/") {
        Some(url.replace("/raw/master/", "/raw/main/"))
    } else if url.contains("/raw/main/") {
        Some(url.replace("/raw/main/", "/raw/master/"))
    } else {
        None
    };

    if let Some(alt) = alt_url {
        crate::log_info!("UPDATER", "URL 404 encountered, attempting alternative branch: {}", alt);
        if let Some(manifest) = try_fetch(&alt)? {
            return Ok(manifest);
        }
    }

    Err(format!("Network request failed: {}: status code 404", url))
}

/// Dual-source race and fallback engine using lightweight threads & mpsc channel
pub fn race_manifest_fetch_sync(
    preferred_source: Option<&str>,
) -> Result<(String, ReleaseManifest), String> {
    // 1. Check local debug mock override (supports both http(s) URL and direct file path)
    if let Some(mock_url) = debug_update_url_override() {
        let trimmed = mock_url.trim();
        if !trimmed.is_empty() {
            if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
                crate::log_info!("UPDATER", "Using debug mock update URL: {}", trimmed);
                let manifest = fetch_single_manifest(trimmed, 5000)?;
                return Ok(("local_mock".to_string(), manifest));
            } else {
                let clean_path = trimmed.trim_start_matches("file:///").replace('/', "\\");
                let path = std::path::PathBuf::from(&clean_path);
                if path.is_file() {
                    crate::log_info!("UPDATER", "Using debug mock local file: {:?}", path);
                    let content = std::fs::read_to_string(&path)
                        .map_err(|e| format!("Failed to read mock file {:?}: {}", path, e))?;
                    let manifest: ReleaseManifest = serde_json::from_str(&content)
                        .map_err(|e| format!("Failed to parse mock JSON: {}", e))?;
                    return Ok(("local_mock".to_string(), manifest));
                }
            }
        }
    }

    // 2. User explicit preference
    if let Some(pref) = preferred_source {
        match pref.to_lowercase().as_str() {
            "gitee" => {
                let m = fetch_single_manifest(DEFAULT_GITEE_URL, 5000)?;
                return Ok(("gitee".to_string(), m));
            }
            "github" => {
                let m = fetch_single_manifest(DEFAULT_GITHUB_URL, 5000)?;
                return Ok(("github".to_string(), m));
            }
            _ => {}
        }
    }

    // 3. Dual-source race: probe both simultaneously with 2000ms threshold
    crate::log_info!("UPDATER", "Starting dual-source race between Gitee and GitHub via lightweight threads...");

    let (tx, rx) = std::sync::mpsc::channel();
    let tx_gitee = tx.clone();
    let tx_github = tx;

    let _ = std::thread::Builder::new()
        .name("updater-race-gitee".to_string())
        .spawn(move || {
            let res = fetch_single_manifest(DEFAULT_GITEE_URL, 2000);
            let _ = tx_gitee.send(("gitee".to_string(), res));
        });

    let _ = std::thread::Builder::new()
        .name("updater-race-github".to_string())
        .spawn(move || {
            let res = fetch_single_manifest(DEFAULT_GITHUB_URL, 2000);
            let _ = tx_github.send(("github".to_string(), res));
        });

    // Wait for the first responder
    let first = rx.recv_timeout(Duration::from_millis(2500))
        .map_err(|_| "Dual-source probe timed out after 2500ms".to_string())?;

    match first.1 {
        Ok(m) => {
            crate::log_info!("UPDATER", "{} responded first and succeeded!", first.0);
            Ok((first.0, m))
        }
        Err(first_err) => {
            let fallback_name = if first.0 == "gitee" { "github" } else { "gitee" };
            crate::log_warn!("UPDATER", "{} fast probe failed ({}). Awaiting fallback {}...", first.0, first_err, fallback_name);
            // Wait for the second responder
            match rx.recv_timeout(Duration::from_millis(3000)) {
                Ok((second_name, Ok(m))) => {
                    crate::log_info!("UPDATER", "Fallback {} succeeded!", second_name);
                    Ok((second_name, m))
                }
                Ok((second_name, Err(second_err))) => {
                    Err(format!("Both sources failed: {} ({}), {} ({})", first.0, first_err, second_name, second_err))
                }
                Err(_) => {
                    // Fallback to a single retry on the alternative with 5000ms
                    let fallback_url = if first.0 == "gitee" { DEFAULT_GITHUB_URL } else { DEFAULT_GITEE_URL };
                    crate::log_info!("UPDATER", "Retrying fallback {} with extended 5000ms timeout...", fallback_name);
                    let m = fetch_single_manifest(fallback_url, 5000)?;
                    Ok((fallback_name.to_string(), m))
                }
            }
        }
    }
}

/// Checks whether an update is available based strictly on SemVer
pub fn evaluate_update(
    local_version: &str,
    _local_commit: &str,
    manifest: &ReleaseManifest,
) -> bool {
    let local_v = match Version::parse(local_version) {
        Ok(v) => v,
        Err(_) => return false,
    };
    let remote_v = match Version::parse(&manifest.version) {
        Ok(v) => v,
        Err(_) => return false,
    };

    // Strict semantic version bump (remote_v > local_v)
    remote_v > local_v
}

fn check_for_updates_sync(preferred_source: Option<&str>) -> Result<UpdateCheckResult, String> {
    let (chosen_source, manifest) = race_manifest_fetch_sync(preferred_source)?;

    let local_version = env!("CARGO_PKG_VERSION");
    let local_commit = option_env!("OPENREVO_GIT_COMMIT").unwrap_or("");
    let dist_type = detect_distribution_type();

    let has_update = evaluate_update(local_version, local_commit, &manifest);

    // Pick suitable asset
    let (asset_key, asset_info) = if dist_type == DistributionType::Installer {
        manifest.assets.get("installer")
            .map(|a| ("installer".to_string(), a))
            .or_else(|| manifest.assets.get("binary_only").map(|a| ("binary_only".to_string(), a)))
            .or_else(|| manifest.assets.get("portable").map(|a| ("portable".to_string(), a)))
            .ok_or_else(|| "No compatible installer asset found in manifest".to_string())?
    } else {
        manifest.assets.get("binary_only")
            .map(|a| ("binary_only".to_string(), a))
            .or_else(|| manifest.assets.get("portable").map(|a| ("portable".to_string(), a)))
            .or_else(|| manifest.assets.get("installer").map(|a| ("installer".to_string(), a)))
            .ok_or_else(|| "No compatible portable asset found in manifest".to_string())?
    };

    // Pick download URL based on chosen source
    let download_url = if chosen_source == "gitee" && !asset_info.sources.gitee.is_empty() {
        asset_info.sources.gitee.clone()
    } else if !asset_info.sources.github.is_empty() {
        asset_info.sources.github.clone()
    } else if let Some(mock_url) = debug_update_url_override() {
        let base = mock_url.rsplit_once('/').map(|(b, _)| b).unwrap_or("http://127.0.0.1:8080");
        format!("{}/{}", base, asset_info.filename)
    } else {
        asset_info.sources.gitee.clone()
    };

    let display_ver = if !manifest.display_version.is_empty() {
        manifest.display_version.clone()
    } else if !manifest.commit.is_empty() {
        format!("v{}-{}", manifest.version, manifest.commit)
    } else {
        format!("v{}", manifest.version)
    };

    Ok(UpdateCheckResult {
        has_update,
        current_version: local_version.to_string(),
        current_commit: local_commit.to_string(),
        latest_version: manifest.version,
        latest_commit: manifest.commit,
        display_version: display_ver,
        notes: manifest.notes,
        chosen_source,
        asset_type: asset_key,
        filename: asset_info.filename.clone(),
        download_url,
        size: asset_info.size,
        sha256: asset_info.sha256.clone(),
    })
}

/// Main entry point for checking updates (asynchronous wrapper using spawn_blocking)
pub async fn check_for_updates(preferred_source: Option<String>) -> Result<UpdateCheckResult, String> {
    tokio::task::spawn_blocking(move || {
        check_for_updates_sync(preferred_source.as_deref())
    })
    .await
    .map_err(|e| format!("Worker thread error: {}", e))?
}



