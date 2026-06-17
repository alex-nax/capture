//! In-app update check against GitHub releases (#48). The GUI checks whether a newer
//! release than the running build exists; if so, the Settings panel offers an **Update**
//! that — ONLY after the user confirms — downloads the notarized .dmg and installs it
//! (replace `/Applications/Capture.app` + relaunch) via a detached updater script.
//!
//! Network + install are best-effort and never block the UI; failures surface as a message.

use std::io::Write as _;
use std::process::{Command, Stdio};

const REPO: &str = "alex-nax/capture";
/// The running build (the GUI crate version == the bundle version).
pub const CURRENT: &str = env!("CARGO_PKG_VERSION");

#[derive(Clone, Debug)]
pub struct UpdateInfo {
    pub version: String,    // e.g. "0.3.0"
    pub dmg_url: String,    // browser_download_url of the .dmg asset
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

/// Query GitHub for the latest release; `Some(info)` only if it's newer than `CURRENT`.
/// Returns `None` on any error or when already up to date (the UI treats both as "no update").
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
    let dmg_url = v
        .get("assets")?
        .as_array()?
        .iter()
        .find_map(|a| {
            let name = a.get("name")?.as_str()?;
            name.ends_with(".dmg").then(|| a.get("browser_download_url")?.as_str().map(String::from))?
        })?;
    Some(UpdateInfo {
        version: tag.trim_start_matches('v').to_string(),
        dmg_url,
    })
}

/// Download the release .dmg and hand off to a detached updater that replaces the app and
/// relaunches it. Blocking (download is ~160 MB) — call on a background thread. The caller
/// has already confirmed with the user.
pub fn download_and_install(info: &UpdateInfo) -> Result<(), String> {
    let resp = agent()
        .get(&info.dmg_url)
        .set("User-Agent", "capture-gui")
        .call()
        .map_err(|e| e.to_string())?;
    let dmg = std::env::temp_dir().join(format!("Capture-update-{}.dmg", info.version));
    {
        let mut f = std::fs::File::create(&dmg).map_err(|e| e.to_string())?;
        std::io::copy(&mut resp.into_reader(), &mut f).map_err(|e| e.to_string())?;
    }
    // The downloaded .dmg is notarized + stapled (GitHub release), so Gatekeeper accepts it
    // and the same Developer-ID signature keeps the Screen Recording (TCC) grant.
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

/// Detached updater: quit the running app, mount the .dmg, replace the installed bundle,
/// relaunch. Self-contained so it survives the app exiting.
const UPDATER_SH: &str = r#"#!/bin/bash
DMG="$1"
APP="/Applications/Capture.app"
# Let the just-spawned process detach, then quit the running app + daemon.
sleep 1
pkill -f "Capture.app/Contents/MacOS/CaptureBar" 2>/dev/null
pkill -f "Capture.app/Contents/Resources/captured/captured" 2>/dev/null
sleep 2
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
