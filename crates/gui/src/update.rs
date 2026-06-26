//! In-app update check against GitHub releases (#48). The GUI checks whether a newer release than the
//! running build exists; if so, Settings offers an **Update** that — ONLY after the user confirms —
//! downloads the OS-specific asset, installs it, and relaunches, via a detached updater:
//!   - macOS:   notarized `.dmg` → replace `/Applications/Capture.app` (detached bash + hdiutil).
//!   - Windows: `CaptureSetup-*-x64.exe` → run it silently, relaunch (detached PowerShell).
//!
//! Network + install are best-effort and never block the UI; failures surface as a message.

use std::io::Write as _;
use std::process::{Command, Stdio};

const REPO: &str = "alex-nax/capture";
/// The running build (the GUI crate version == the bundle version).
pub const CURRENT: &str = env!("CARGO_PKG_VERSION");

#[derive(Clone, Debug)]
pub struct UpdateInfo {
    pub version: String,   // e.g. "0.3.0"
    pub asset_url: String, // browser_download_url of the OS-specific asset (.dmg / .exe)
}

fn parse_semver(s: &str) -> Option<(u32, u32, u32)> {
    let s = s.trim().trim_start_matches('v');
    let mut it = s.split(|c: char| c == '.' || c == '-' || c == '+');
    let major = it.next()?.parse().ok()?;
    let minor = it.next().unwrap_or("0").parse().ok()?;
    let patch = it.next().unwrap_or("0").parse().ok()?;
    Some((major, minor, patch))
}

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout(std::time::Duration::from_secs(15))
        .build()
}

/// The release asset that matches this OS (macOS `.dmg`; Windows `CaptureSetup*.exe`).
#[cfg(target_os = "macos")]
fn asset_matches(name: &str) -> bool {
    name.ends_with(".dmg")
}
#[cfg(target_os = "windows")]
fn asset_matches(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n.starts_with("capturesetup") && n.ends_with(".exe")
}
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn asset_matches(_name: &str) -> bool {
    false
}

/// Query GitHub for the latest release; `Some(info)` only if it's newer than `CURRENT` AND carries an
/// asset for this OS. Returns `None` on any error or when already up to date.
pub fn check() -> Option<UpdateInfo> {
    let url = format!("https://api.github.com/repos/{REPO}/releases/latest");
    let resp = agent()
        .get(&url)
        .set("User-Agent", "capture-gui")
        .set("Accept", "application/vnd.github+json")
        .call()
        .ok()?;
    let v: serde_json::Value = resp.into_json().ok()?;
    let tag = v.get("tag_name")?.as_str()?.to_string();
    let (cur, latest) = (parse_semver(CURRENT)?, parse_semver(&tag)?);
    if latest <= cur {
        return None;
    }
    let asset_url = v
        .get("assets")?
        .as_array()?
        .iter()
        .find_map(|a| {
            let name = a.get("name")?.as_str()?;
            asset_matches(name).then(|| a.get("browser_download_url")?.as_str().map(String::from))?
        })?;
    Some(UpdateInfo {
        version: tag.trim_start_matches('v').to_string(),
        asset_url,
    })
}

/// Download the OS asset and hand off to a detached updater that installs it + relaunches. Blocking
/// (the download is large) — call on a background thread. The caller has already confirmed.
/// `progress(downloaded_bytes, total_bytes)` is called after each chunk so the UI can show a bar;
/// `total` is 0 when the server doesn't send `Content-Length`.
pub fn download_and_install<F: Fn(u64, u64) + Send>(
    info: &UpdateInfo,
    progress: F,
) -> Result<(), String> {
    let resp = agent()
        .get(&info.asset_url)
        .set("User-Agent", "capture-gui")
        .call()
        .map_err(|e| e.to_string())?;
    #[cfg(target_os = "macos")]
    {
        install_macos(info, resp, progress)
    }
    #[cfg(target_os = "windows")]
    {
        install_windows(info, resp, progress)
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = (info, resp, progress);
        Err("in-app update is not supported on this platform".into())
    }
}

/// Stream the response body into `path` in 64 KiB chunks, reporting `(downloaded, total)` after each.
/// `total` comes from `Content-Length` (0 when absent). Shared by the per-OS installers.
#[cfg(any(target_os = "macos", target_os = "windows"))]
fn download_to<F: Fn(u64, u64)>(
    resp: ureq::Response,
    path: &std::path::Path,
    progress: &F,
) -> Result<(), String> {
    use std::io::Read as _;
    let total = resp
        .header("Content-Length")
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(0);
    let mut reader = resp.into_reader();
    let mut f = std::fs::File::create(path).map_err(|e| e.to_string())?;
    let mut buf = [0u8; 65536];
    let mut downloaded: u64 = 0;
    progress(0, total);
    loop {
        let n = reader.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        f.write_all(&buf[..n]).map_err(|e| e.to_string())?;
        downloaded += n as u64;
        progress(downloaded, total);
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn install_macos<F: Fn(u64, u64)>(
    info: &UpdateInfo,
    resp: ureq::Response,
    progress: F,
) -> Result<(), String> {
    let dmg = std::env::temp_dir().join(format!("Capture-update-{}.dmg", info.version));
    download_to(resp, &dmg, &progress)?;
    // The downloaded .dmg is notarized + stapled, so Gatekeeper accepts it and the same Developer-ID
    // signature keeps the Screen Recording (TCC) grant.
    let script = std::env::temp_dir().join("capture-updater.sh");
    {
        let mut f = std::fs::File::create(&script).map_err(|e| e.to_string())?;
        f.write_all(UPDATER_SH.as_bytes()).map_err(|e| e.to_string())?;
    }
    Command::new("/bin/bash")
        .arg(&script)
        .arg(&dmg)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(target_os = "windows")]
fn install_windows<F: Fn(u64, u64)>(
    info: &UpdateInfo,
    resp: ureq::Response,
    progress: F,
) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    let exe = std::env::temp_dir().join(format!("CaptureSetup-update-{}.exe", info.version));
    download_to(resp, &exe, &progress)?;
    let script = std::env::temp_dir().join("capture-updater.ps1");
    {
        let mut f = std::fs::File::create(&script).map_err(|e| e.to_string())?;
        f.write_all(UPDATER_PS1.as_bytes()).map_err(|e| e.to_string())?;
    }
    // Detached + windowless so the updater survives the app it's about to stop.
    Command::new("powershell")
        .args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-WindowStyle", "Hidden", "-File"])
        .arg(&script)
        .arg(&exe)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(0x0800_0000 | 0x0000_0200) // CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP
        .spawn()
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Detached macOS updater: stop the WHOLE app (agent + window + daemon), mount the .dmg, replace the
/// bundle, relaunch. Mirrors the Windows `UPDATER_PS1`. Kills all three (the old code left
/// `capture-gui` running) and resets `daemon.json` so the relaunched agent spawns a FRESH daemon
/// instead of adopting the old one still answering `/v1/health` (which left the daemon un-updated).
#[cfg(target_os = "macos")]
const UPDATER_SH: &str = r#"#!/bin/bash
DMG="$1"
APP="/Applications/Capture.app"
sleep 1
# Stop the agent, the GUI window, AND the daemon — everything under the bundle.
pkill -9 -f "Capture.app/Contents" 2>/dev/null
# Wait for the daemon to actually exit so it isn't pinning the bundle or answering health.
for _ in $(seq 1 15); do pgrep -f "Capture.app/Contents/Resources/captured/captured" >/dev/null || break; sleep 1; done
# Drop daemon discovery so the relaunched agent starts a new daemon (won't adopt a stale one).
rm -f "$HOME/.capture/daemon.json"
MNT=$(hdiutil attach "$DMG" -nobrowse -noverify 2>/dev/null | grep -o '/Volumes/[^[:cntrl:]]*' | tail -1)
if [ -n "$MNT" ] && [ -d "$MNT/Capture.app" ]; then
  rm -rf "$APP"
  cp -R "$MNT/Capture.app" "$APP"
  hdiutil detach "$MNT" >/dev/null 2>&1
  xattr -dr com.apple.quarantine "$APP" 2>/dev/null
  open "$APP"
fi
rm -f "$DMG"
"#;

/// Detached Windows updater: stop the agent/window/daemon, run the installer silently (Inno upgrades
/// the existing per-user install in place by AppId), relaunch the tray agent.
#[cfg(target_os = "windows")]
const UPDATER_PS1: &str = r#"param([string]$Installer)
Start-Sleep -Seconds 1
Get-Process Capture, capture-gui, captured -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
Start-Sleep -Seconds 2
Start-Process $Installer -ArgumentList '/VERYSILENT','/SUPPRESSMSGBOXES','/NORESTART','/SP-' -Wait
$exe = Join-Path $env:LOCALAPPDATA 'Programs\Capture\Capture.exe'
if (Test-Path $exe) { Start-Process $exe }
Remove-Item $Installer -Force -ErrorAction SilentlyContinue
"#;
