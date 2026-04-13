use std::env;
use std::io::{self, IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use serde::Deserialize;

const DEFAULT_REPO: &str = "alentra-dev/contynu";
const CHECK_TIMEOUT_MS: u64 = 1200;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StartupUpdateOutcome {
    Continue,
    ExitAfterManualPrompt,
    ExitAfterAutoUpdate,
}

#[derive(Debug, Clone)]
struct RuntimePlatform {
    os: &'static str,
    arch: &'static str,
    archive_ext: &'static str,
    installer_asset: &'static str,
}

#[derive(Debug, Deserialize)]
struct ReleaseInfo {
    tag_name: String,
    #[serde(default)]
    assets: Vec<ReleaseAsset>,
}

#[derive(Debug, Deserialize)]
struct ReleaseAsset {
    name: String,
}

#[derive(Debug, Clone)]
struct UpdatePlan {
    tag_name: String,
    platform_asset: String,
    installer_asset: String,
    manual_command: String,
}

pub fn maybe_handle_startup_update(skip_interactive_prompt: bool) -> Result<StartupUpdateOutcome> {
    if skip_interactive_prompt || env_flag("CONTYNU_SKIP_UPDATE_CHECK") {
        return Ok(StartupUpdateOutcome::Continue);
    }

    let platform = match current_platform() {
        Some(platform) => platform,
        None => return Ok(StartupUpdateOutcome::Continue),
    };

    let Some(update) = fetch_update_plan(&platform)? else {
        return Ok(StartupUpdateOutcome::Continue);
    };

    let interactive = env_flag("CONTYNU_FORCE_INTERACTIVE_UPDATE_PROMPT")
        || (io::stdin().is_terminal() && io::stderr().is_terminal());
    if !interactive {
        eprintln!(
            "Contynu {} is available for this {} environment.",
            update.tag_name, platform.os
        );
        eprintln!("Manual update command:");
        eprintln!("{}", update.manual_command);
        return Ok(StartupUpdateOutcome::Continue);
    }

    eprintln!(
        "A newer Contynu release is available for this {} environment: {}",
        platform.os, update.tag_name
    );
    eprintln!("Current version: v{}", env!("CARGO_PKG_VERSION"));
    eprintln!("Platform asset: {}", update.platform_asset);
    eprintln!("Choose an update action:");
    eprintln!("  [m] manual update");
    eprintln!("  [a] auto update now");
    eprintln!("  [s] skip for this launch");
    eprint!("Selection: ");
    io::stderr().flush()?;

    let mut choice = String::new();
    io::stdin().read_line(&mut choice)?;
    match choice.trim().to_ascii_lowercase().as_str() {
        "m" | "manual" => {
            eprintln!("Run this command to update:");
            eprintln!("{}", update.manual_command);
            Ok(StartupUpdateOutcome::ExitAfterManualPrompt)
        }
        "a" | "auto" => {
            run_auto_update(&platform, &update)?;
            eprintln!(
                "Contynu was updated to {}. Relaunch the command to use the new binary.",
                update.tag_name
            );
            Ok(StartupUpdateOutcome::ExitAfterAutoUpdate)
        }
        _ => Ok(StartupUpdateOutcome::Continue),
    }
}

fn fetch_update_plan(platform: &RuntimePlatform) -> Result<Option<UpdatePlan>> {
    let release = match fetch_latest_release()? {
        Some(release) => release,
        None => return Ok(None),
    };

    if !is_newer_release(&release.tag_name, env!("CARGO_PKG_VERSION")) {
        return Ok(None);
    }

    let platform_asset = platform_asset_name(platform);
    if !release
        .assets
        .iter()
        .any(|asset| asset.name == platform_asset)
    {
        return Ok(None);
    }

    if !release
        .assets
        .iter()
        .any(|asset| asset.name == platform.installer_asset)
    {
        return Ok(None);
    }

    Ok(Some(UpdatePlan {
        tag_name: release.tag_name.clone(),
        platform_asset,
        installer_asset: platform.installer_asset.to_string(),
        manual_command: manual_update_command(platform, &release.tag_name)?,
    }))
}

fn fetch_latest_release() -> Result<Option<ReleaseInfo>> {
    let api_url = release_api_url();
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_millis(CHECK_TIMEOUT_MS))
        .timeout_read(Duration::from_millis(CHECK_TIMEOUT_MS))
        .timeout_write(Duration::from_millis(CHECK_TIMEOUT_MS))
        .user_agent(&format!("contynu/{}", env!("CARGO_PKG_VERSION")))
        .build();

    let response = match agent.get(&api_url).set("Accept", "application/json").call() {
        Ok(response) => response,
        Err(ureq::Error::Status(404, _)) => return Ok(None),
        Err(ureq::Error::Status(_, response)) => {
            let status = response.status();
            eprintln!("[contynu] Release check skipped: update endpoint returned HTTP {status}");
            return Ok(None);
        }
        Err(err) => {
            eprintln!("[contynu] Release check skipped: {err}");
            return Ok(None);
        }
    };

    let release: ReleaseInfo = response
        .into_json()
        .context("failed to decode latest release metadata")?;
    Ok(Some(release))
}

fn current_platform() -> Option<RuntimePlatform> {
    let os = match env::consts::OS {
        "linux" => "linux",
        "macos" => "macos",
        "windows" => "windows",
        _ => return None,
    };
    let arch = match env::consts::ARCH {
        "x86_64" => "x86_64",
        "aarch64" => "aarch64",
        _ => return None,
    };

    Some(RuntimePlatform {
        os,
        arch,
        archive_ext: if os == "windows" { "zip" } else { "tar.gz" },
        installer_asset: if os == "windows" {
            "install.ps1"
        } else {
            "install.sh"
        },
    })
}

fn platform_asset_name(platform: &RuntimePlatform) -> String {
    format!(
        "contynu-{}-{}.{}",
        platform.os, platform.arch, platform.archive_ext
    )
}

fn release_api_url() -> String {
    env::var("CONTYNU_RELEASE_API_URL").unwrap_or_else(|_| {
        format!(
            "https://api.github.com/repos/{}/releases/latest",
            env::var("CONTYNU_REPO").unwrap_or_else(|_| DEFAULT_REPO.to_string())
        )
    })
}

fn release_download_base_url() -> String {
    env::var("CONTYNU_RELEASE_DOWNLOAD_BASE_URL").unwrap_or_else(|_| {
        format!(
            "https://github.com/{}/releases/download",
            env::var("CONTYNU_REPO").unwrap_or_else(|_| DEFAULT_REPO.to_string())
        )
    })
}

fn manual_update_command(platform: &RuntimePlatform, tag_name: &str) -> Result<String> {
    let install_dir = install_dir()?;
    let installer_url = format!(
        "{}/{}/{}",
        release_download_base_url(),
        tag_name,
        platform.installer_asset
    );

    Ok(if platform.os == "windows" {
        format!(
            "$env:CONTYNU_INSTALL_DIR='{}'; irm {} | iex",
            install_dir.display(),
            installer_url
        )
    } else {
        format!(
            "curl -fsSL {} | CONTYNU_INSTALL_DIR=\"{}\" sh",
            installer_url,
            install_dir.display()
        )
    })
}

fn install_dir() -> Result<PathBuf> {
    if let Some(dir) = env::var_os("CONTYNU_INSTALL_DIR") {
        return Ok(PathBuf::from(dir));
    }

    let exe = env::current_exe().context("failed to resolve current contynu executable path")?;
    exe.parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| anyhow!("current contynu executable has no parent directory"))
}

fn run_auto_update(platform: &RuntimePlatform, update: &UpdatePlan) -> Result<()> {
    let installer_url = format!(
        "{}/{}/{}",
        release_download_base_url(),
        update.tag_name,
        update.installer_asset
    );
    let install_dir = install_dir()?;

    if platform.os == "windows" {
        let script = download_text(&installer_url)?;
        let tmp = env::temp_dir().join(format!("contynu-update-{}.ps1", std::process::id()));
        std::fs::write(&tmp, script)?;
        let status = Command::new("powershell")
            .arg("-NoProfile")
            .arg("-ExecutionPolicy")
            .arg("Bypass")
            .arg("-File")
            .arg(&tmp)
            .env("CONTYNU_INSTALL_DIR", &install_dir)
            .status()
            .with_context(|| format!("failed to run {}", tmp.display()))?;
        let _ = std::fs::remove_file(&tmp);
        if !status.success() {
            return Err(anyhow!(
                "auto update failed while running {}",
                update.installer_asset
            ));
        }
        return Ok(());
    }

    let script = download_text(&installer_url)?;
    let mut child = Command::new("sh")
        .env("CONTYNU_INSTALL_DIR", &install_dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()
        .context("failed to spawn sh for Contynu auto update")?;
    child
        .stdin
        .as_mut()
        .context("failed to open stdin for Contynu auto update")?
        .write_all(script.as_bytes())?;
    let status = child.wait()?;
    if !status.success() {
        return Err(anyhow!(
            "auto update failed while running {}",
            update.installer_asset
        ));
    }
    Ok(())
}

fn download_text(url: &str) -> Result<String> {
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_millis(CHECK_TIMEOUT_MS))
        .timeout_read(Duration::from_millis(CHECK_TIMEOUT_MS))
        .timeout_write(Duration::from_millis(CHECK_TIMEOUT_MS))
        .user_agent(&format!("contynu/{}", env!("CARGO_PKG_VERSION")))
        .build();
    let response = agent
        .get(url)
        .call()
        .with_context(|| format!("failed to download {url}"))?;
    let mut body = String::new();
    response
        .into_reader()
        .read_to_string(&mut body)
        .with_context(|| format!("failed to read response from {url}"))?;
    Ok(body)
}

fn is_newer_release(candidate: &str, current: &str) -> bool {
    parse_version(candidate) > parse_version(current)
}

fn parse_version(version: &str) -> (u64, u64, u64) {
    let trimmed = version.trim_start_matches('v');
    let mut parts = trimmed.split('.');
    let major = parts.next().and_then(|v| v.parse().ok()).unwrap_or(0);
    let minor = parts.next().and_then(|v| v.parse().ok()).unwrap_or(0);
    let patch = parts.next().and_then(|v| v.parse().ok()).unwrap_or(0);
    (major, minor, patch)
}

fn env_flag(name: &str) -> bool {
    env::var(name)
        .map(|value| matches!(value.as_str(), "1" | "true" | "TRUE" | "yes" | "YES"))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_comparison_ignores_v_prefix() {
        assert!(is_newer_release("v0.6.0", "0.5.0"));
        assert!(is_newer_release("0.6.1", "v0.6.0"));
        assert!(!is_newer_release("v0.5.0", "0.5.0"));
    }

    #[test]
    fn current_platform_asset_name_matches_release_convention() {
        let platform = current_platform().unwrap();
        let asset = platform_asset_name(&platform);
        assert!(asset.starts_with("contynu-"));
        assert!(asset.ends_with(platform.archive_ext));
    }
}
