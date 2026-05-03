use std::path::PathBuf;

/// Returns the canonical FrogClawClient home directory and ensures it exists.
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
