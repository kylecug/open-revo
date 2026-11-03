// OpenRevo - Auto Updater Downloader and Safe Application Engine
// Pure ASCII compliant

use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Instant;
use sha2::{Digest, Sha256};
use tauri::Emitter;
use super::types::DownloadProgressPayload;

/// Public half of the release signing key, embedded at build time (base64 line of minisign.pub).
const UPDATE_PUBLIC_KEY: Option<&str> = option_env!("OPENREVO_UPDATE_PUBKEY");

/// Verifies the downloaded file against "<download_url>.minisig" using the embedded public key.
/// Builds with an embedded key always require a valid signature. The SHA256 in the manifest
/// comes from the same channel as the file, so only this check proves who published it.
fn verify_update_signature(agent: &ureq::Agent, download_url: &str, file_path: &Path) -> Result<(), String> {
    let public_key = match UPDATE_PUBLIC_KEY.map(str::trim).filter(|key| !key.is_empty()) {
        Some(key) => key,
        None => {
            crate::log_warn!("UPDATER", "This build has no update public key; skipping signature verification.");
            return Ok(());
        }
    };

    let public_key = minisign_verify::PublicKey::from_base64(public_key)
        .map_err(|e| format!("Embedded update public key is invalid: {}", e))?;

    let signature_url = format!("{}.minisig", download_url);
    let signature_text = agent.get(&signature_url)
        .call()
        .map_err(|e| format!("Failed to download update signature: {}", e))?
        .into_string()
        .map_err(|e| format!("Failed to read update signature: {}", e))?;

    let signature = minisign_verify::Signature::decode(&signature_text)
        .map_err(|e| format!("Update signature is malformed: {}", e))?;

    let contents = std::fs::read(file_path)
        .map_err(|e| format!("Failed to read downloaded update: {}", e))?;

    public_key.verify(&contents, &signature, false)
        .map_err(|e| format!("Update signature verification failed: {}", e))
}

/// Downloads update asset with real-time progress events, SHA256 and signature verification
pub async fn download_update_asset<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    download_url: &str,
    expected_sha256: &str,
    filename: &str,
) -> Result<PathBuf, String> {
    let app_handle = app.clone();
    let url = download_url.to_string();
    let expected_hash = expected_sha256.to_string();
    let fname = filename.to_string();

    tokio::task::spawn_blocking(move || {
        download_update_asset_sync(&app_handle, &url, &expected_hash, &fname)
    })
    .await
    .map_err(|e| format!("Worker thread error: {}", e))?
}

fn download_update_asset_sync<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    download_url: &str,
    expected_sha256: &str,
    filename: &str,
) -> Result<PathBuf, String> {
    let mut builder = ureq::builder()
        .timeout_connect(std::time::Duration::from_secs(15))
        .timeout_read(std::time::Duration::from_secs(60))
        .user_agent("OpenRevo-Updater")
        .redirects(5);

    match ureq::native_tls::TlsConnector::new() {
        Ok(tls) => {
            builder = builder.tls_connector(std::sync::Arc::new(tls));
        }
        Err(e) => {
            crate::log_error!("UPDATER", "Failed to initialize native TLS connector: {}", e);
        }
    }

    let agent = builder.build();

    let temp_update_dir = std::env::temp_dir().join("openrevo_update");
    std::fs::create_dir_all(&temp_update_dir).map_err(|e| format!("Failed to create temp dir: {}", e))?;

    let partial_file_path = temp_update_dir.join(format!("{}.downloading", filename));
    let final_file_path = temp_update_dir.join(filename);

    crate::log_info!("UPDATER", "Downloading update via ureq from: {}", download_url);

    let resp = agent.get(download_url)
        .call()
        .map_err(|e| format!("Download request failed: {}", e))?;

    if resp.status() != 200 {
        return Err(format!("Download failed with HTTP {}", resp.status()));
    }

    let total_size: u64 = resp.header("Content-Length")
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(0);

    let mut reader = resp.into_reader();
    let mut file = File::create(&partial_file_path).map_err(|e| format!("Failed to create file: {}", e))?;
    let mut hasher = Sha256::new();
    let mut downloaded: u64 = 0;

    let start_time = Instant::now();
    let mut last_emit = Instant::now();
    let mut buffer = [0u8; 32768]; // 32KB buffer

    loop {
        let n = reader.read(&mut buffer).map_err(|e| format!("Read chunk failed: {}", e))?;
        if n == 0 {
            break;
        }
        file.write_all(&buffer[..n]).map_err(|e| format!("Write failed: {}", e))?;
        hasher.update(&buffer[..n]);
        downloaded += n as u64;

        if last_emit.elapsed().as_millis() >= 100 || (total_size > 0 && downloaded == total_size) {
            last_emit = Instant::now();
            let elapsed_secs = start_time.elapsed().as_secs_f64();
            let speed = if elapsed_secs > 0.0 {
                (downloaded as f64 / elapsed_secs) as u64
            } else {
                0
            };
            let percentage = if total_size > 0 {
                (downloaded as f64 / total_size as f64) * 100.0
            } else {
                0.0
            };

            let _ = app.emit("updater-progress", DownloadProgressPayload {
                downloaded,
                total: total_size,
                percentage,
                speed_bytes_per_sec: speed,
            });
        }
    }

    file.flush().map_err(|e| format!("Flush failed: {}", e))?;
    drop(file);

    // Verify SHA256 checksum
    let calculated_sha256 = format!("{:x}", hasher.finalize());
    crate::log_info!("UPDATER", "Download complete. Expected SHA: {}, Calculated: {}", expected_sha256, calculated_sha256);

    if expected_sha256.trim().is_empty() {
        let _ = std::fs::remove_file(&partial_file_path);
        return Err("Integrity check failed! Update manifest does not provide a SHA256 hash".to_string());
    }

    if !calculated_sha256.eq_ignore_ascii_case(expected_sha256.trim()) {
        let _ = std::fs::remove_file(&partial_file_path);
        return Err(format!("Integrity check failed! Expected hash: {}, got: {}", expected_sha256, calculated_sha256));
    }

    if let Err(e) = verify_update_signature(&agent, download_url, &partial_file_path) {
        let _ = std::fs::remove_file(&partial_file_path);
        return Err(e);
    }

    // Move to final path
    if final_file_path.exists() {
        let _ = std::fs::remove_file(&final_file_path);
    }
    std::fs::rename(&partial_file_path, &final_file_path).map_err(|e| format!("Failed to finalize file: {}", e))?;

    Ok(final_file_path)
}

/// Applies update by either launching installer silently or hot-swapping portable binary
pub fn apply_update(downloaded_file: &Path, is_installer: bool) -> Result<(), String> {
    if !downloaded_file.exists() {
        return Err("Update file not found".to_string());
    }

    if is_installer {
        crate::log_info!("UPDATER", "Launching NSIS installer in silent mode: {:?}", downloaded_file);
        // Launch installer with /S for silent upgrade
        std::process::Command::new(downloaded_file)
            .arg("/S")
            .spawn()
            .map_err(|e| format!("Failed to spawn installer: {}", e))?;

        std::process::exit(0);
    } else {
        // Portable mode: Hot-swap via detached pure-ASCII batch script
        let current_exe = std::env::current_exe().map_err(|e| format!("Failed to locate current exe: {}", e))?;
        let temp_dir = downloaded_file.parent().unwrap_or_else(|| Path::new("."));
        let bat_path = temp_dir.join("apply_update.bat");

        let current_exe_str = current_exe.to_string_lossy().to_string();
        let new_file_str = downloaded_file.to_string_lossy().to_string();

        let bat_content = format!(
            "@echo off\r\ntimeout /t 1 /nobreak >nul\r\ncopy /y \"{}\" \"{}\"\r\nstart \"\" \"{}\"\r\ndel \"%~f0\"\r\n",
            new_file_str, current_exe_str, current_exe_str
        );

        std::fs::write(&bat_path, bat_content.as_bytes()).map_err(|e| format!("Failed to write updater script: {}", e))?;

        crate::log_info!("UPDATER", "Executing update script and terminating self...");

        #[cfg(target_os = "windows")]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x08000000;
            std::process::Command::new("cmd.exe")
                .args(["/c", &bat_path.to_string_lossy()])
                .creation_flags(CREATE_NO_WINDOW)
                .spawn()
                .map_err(|e| format!("Failed to execute update script: {}", e))?;
        }

        std::process::exit(0);
    }
}
