//! Install the bundled `capture` skill into a coding agent's skills home, so the
//! agent (Claude Code, Codex, …) can drive capture-mcp from any project.
//!
//! Source: the skill is bundled at `Capture.app/Contents/Resources/skill` in the
//! packaged app, or read from `<repo>/skills/capture` in a dev build. Destination:
//! `~/<agent.home_subdir>/capture` (e.g. `~/.claude/skills/capture`).

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

pub struct Agent {
    pub label: &'static str,
    pub home_subdir: &'static str,
}

/// Installed state of the skill for one agent, vs the bundled copy.
#[derive(Clone, Copy, PartialEq)]
pub enum SkillStatus {
    NotInstalled,
    UpToDate,
    UpdateAvailable, // installed, but differs from the bundled skill (we shipped an update)
}

/// Compare the installed skill against the bundled one (content hash of all files).
pub fn status(agent: &Agent) -> SkillStatus {
    let dest = match dirs::home_dir() {
        Some(h) => h.join(agent.home_subdir).join("capture"),
        None => return SkillStatus::NotInstalled,
    };
    if !dest.join("SKILL.md").exists() {
        return SkillStatus::NotInstalled;
    }
    match skill_source() {
        Some(src) if dir_hash(&src) != dir_hash(&dest) => SkillStatus::UpdateAvailable,
        _ => SkillStatus::UpToDate, // identical, or no source to compare against
    }
}

fn dir_hash(root: &Path) -> u64 {
    let mut files: Vec<(String, Vec<u8>)> = Vec::new();
    collect_files(root, root, &mut files);
    files.sort_by(|a, b| a.0.cmp(&b.0));
    let mut h = DefaultHasher::new();
    for (rel, bytes) in &files {
        rel.hash(&mut h);
        bytes.hash(&mut h);
    }
    h.finish()
}

fn collect_files(root: &Path, dir: &Path, out: &mut Vec<(String, Vec<u8>)>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for entry in rd.flatten() {
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if name_str == "__pycache__" {
            continue;
        }
        let p = entry.path();
        if p.is_dir() {
            collect_files(root, &p, out);
        } else if !name_str.ends_with(".pyc") {
            if let (Ok(rel), Ok(bytes)) = (p.strip_prefix(root), std::fs::read(&p)) {
                out.push((rel.to_string_lossy().into_owned(), bytes));
            }
        }
    }
}

pub const AGENTS: &[Agent] = &[
    Agent { label: "Claude Code", home_subdir: ".claude/skills" },
    Agent { label: "Codex", home_subdir: ".codex/skills" },
];

/// Locate the skill source: bundled in the .app, else the repo (dev build).
fn skill_source() -> Option<PathBuf> {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            // macOS: Capture.app/Contents/Resources/skill (capture-gui in MacOS/).
            // Windows/other: skill\ beside the exe at the install root.
            #[cfg(target_os = "macos")]
            let bundled = dir.join("../Resources/skill");
            #[cfg(not(target_os = "macos"))]
            let bundled = dir.join("skill");
            if bundled.join("SKILL.md").exists() {
                return Some(bundled);
            }
        }
    }
    // Dev build: <gui crate>/../skills/capture (path baked at compile time).
    let dev = Path::new(env!("CARGO_MANIFEST_DIR")).join("../skills/capture");
    if dev.join("SKILL.md").exists() {
        return Some(dev);
    }
    None
}

/// Copy the skill into `~/<agent.home_subdir>/capture` (clean replace). Returns
/// the install path. Excludes `__pycache__` / `*.pyc`.
pub fn install(agent: &Agent) -> Result<PathBuf, String> {
    let src = skill_source()
        .ok_or("skill source not found (not bundled in the app, not in a repo checkout)")?;
    let home = dirs::home_dir().ok_or("could not resolve the home directory")?;
    let dest = home.join(agent.home_subdir).join("capture");
    if dest.exists() {
        std::fs::remove_dir_all(&dest).map_err(|e| format!("remove old skill: {e}"))?;
    }
    copy_dir(&src, &dest).map_err(|e| format!("copy skill: {e}"))?;
    Ok(dest)
}

fn copy_dir(src: &Path, dest: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dest)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if name_str == "__pycache__" {
            continue;
        }
        let from = entry.path();
        let to = dest.join(&name);
        if from.is_dir() {
            copy_dir(&from, &to)?;
        } else if !name_str.ends_with(".pyc") {
            std::fs::copy(&from, &to)?;
        }
    }
    Ok(())
}
