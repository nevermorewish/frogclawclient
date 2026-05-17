use std::path::PathBuf;

/// Returns the canonical FrogClaw home directory and ensures it exists.
///
/// - macOS / Linux: `~/.frogclaw/`
/// - Windows:       `%USERPROFILE%\.frogclaw\`
///
/// Panics if the home directory cannot be determined.
pub fn frogclaw_home() -> PathBuf {
    user_home().join(".frogclaw")
}

/// Returns the default project workspace under the FrogClaw config home.
pub fn default_workspace() -> PathBuf {
    frogclaw_home().join("workspace")
}

fn user_home() -> PathBuf {
    #[cfg(not(windows))]
    let home = std::env::var("HOME").expect("HOME env var not set");
    #[cfg(windows)]
    let home = std::env::var("USERPROFILE").expect("USERPROFILE env var not set");

    PathBuf::from(home)
}

const ROTATED_LOGS: &[&str] = &["memory.log", "platform-sidecar.log", "ai-agent.log"];

/// Archive existing top-level log files under `~/.frogclaw/backlogs/` so each
/// app launch starts with fresh log files. Failures are logged but never abort
/// startup (a held file lock or permission denial is recoverable — the next
/// rotation will retry).
pub fn rotate_startup_logs() {
    let home = frogclaw_home();
    let backlogs = home.join("backlogs");
    if let Err(err) = std::fs::create_dir_all(&backlogs) {
        tracing::warn!("rotate_startup_logs: create backlogs dir failed: {}", err);
        return;
    }
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S").to_string();
    for name in ROTATED_LOGS {
        let src = home.join(name);
        if !src.is_file() {
            continue;
        }
        let (stem, ext) = name
            .rsplit_once('.')
            .map(|(s, e)| (s.to_string(), format!(".{e}")))
            .unwrap_or_else(|| (name.to_string(), String::new()));
        let dest = backlogs.join(format!("{stem}-{stamp}{ext}"));
        if let Err(err) = std::fs::rename(&src, &dest) {
            tracing::warn!(
                "rotate_startup_logs: move {} -> {} failed: {}",
                src.display(),
                dest.display(),
                err
            );
        }
    }
}
