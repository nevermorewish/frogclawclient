use serde::Serialize;
use std::io::Write;
use std::process::Command;

#[derive(Debug, Clone, Serialize)]
pub struct ToolStatus {
    pub id: String,
    pub name: String,
    pub installed: bool,
    pub version: Option<String>,
    pub path: Option<String>,
    pub installable: bool,
    pub needs_upgrade: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct HomeToolsStatus {
    pub tools: Vec<ToolStatus>,
}

#[derive(Debug, Clone, Serialize)]
pub struct InstallResult {
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
    pub message: String,
    pub log_file: Option<String>,
}

#[cfg(target_os = "windows")]
const CREATE_NO_WINDOW: u32 = 0x08000000;

const MIN_NODE_FOR_CLI: (u32, u32) = (18, 0);

fn frogclaw_log_path() -> Option<std::path::PathBuf> {
    Some(crate::paths::frogclaw_home().join("install.log"))
}

fn write_log(msg: &str) {
    if let Some(path) = frogclaw_log_path() {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
            let ts = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
            let _ = writeln!(f, "[{}] {}", ts, msg);
        }
    }
}

#[cfg(target_os = "windows")]
fn ensure_windows_path() {
    let current = std::env::var("PATH").unwrap_or_default();
    let mut extra: Vec<String> = Vec::new();

    if let Ok(local_app) = std::env::var("LOCALAPPDATA") {
        let windows_apps = format!("{}\\Microsoft\\WindowsApps", local_app);
        if std::path::Path::new(&windows_apps).exists() {
            extra.push(windows_apps);
        }
        let npm = format!("{}\\npm", local_app.replace("Local", "Roaming").replace("local", "Roaming"));
        if std::path::Path::new(&npm).exists() {
            extra.push(npm);
        }
    }
    if let Ok(program_files) = std::env::var("ProgramFiles") {
        for dir in [
            format!("{}\\nodejs", program_files),
            format!("{}\\Git\\cmd", program_files),
        ] {
            if std::path::Path::new(&dir).exists() {
                extra.push(dir);
            }
        }
    }
    if let Ok(user_profile) = std::env::var("USERPROFILE") {
        for dir in [
            format!("{}\\AppData\\Roaming\\npm", user_profile),
            format!("{}\\AppData\\Roaming\\nvm", user_profile),
            format!("{}\\.fnm", user_profile),
        ] {
            if std::path::Path::new(&dir).exists() {
                extra.push(dir);
            }
        }
    }
    for dir in ["C:\\ProgramData\\nvm", "C:\\Program Files\\nodejs"] {
        if std::path::Path::new(dir).exists() {
            extra.push(dir.to_string());
        }
    }

    let filtered: Vec<String> = extra
        .into_iter()
        .filter(|p| !current.to_lowercase().contains(&p.to_lowercase()))
        .collect();
    if !filtered.is_empty() {
        std::env::set_var("PATH", format!("{};{}", filtered.join(";"), current));
        write_log(&format!("PATH extended with: {}", filtered.join("; ")));
    }
}

#[cfg(not(target_os = "windows"))]
fn ensure_unix_path() {
    let current = std::env::var("PATH").unwrap_or_default();
    let mut extra = Vec::new();
    for dir in [
        "/usr/local/bin",
        "/opt/homebrew/bin",
        "/home/linuxbrew/.linuxbrew/bin",
        "/usr/bin",
        "/bin",
        "/snap/bin",
    ] {
        if std::path::Path::new(dir).is_dir() && !current.split(':').any(|p| p == dir) {
            extra.push(dir.to_string());
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        for dir in [
            format!("{home}/.local/bin"),
            format!("{home}/.npm-global/bin"),
            format!("{home}/n/bin"),
        ] {
            if std::path::Path::new(&dir).is_dir() {
                extra.push(dir);
            }
        }
    }
    if !extra.is_empty() {
        std::env::set_var("PATH", format!("{}:{}", extra.join(":"), current));
        write_log(&format!("PATH extended with: {}", extra.join(":")));
    }
}

fn prepare_path() {
    #[cfg(target_os = "windows")]
    ensure_windows_path();
    #[cfg(not(target_os = "windows"))]
    ensure_unix_path();
}

#[cfg(target_os = "windows")]
fn is_reparse_point(p: &std::path::Path) -> bool {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
    std::fs::symlink_metadata(p)
        .map(|m| m.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0)
        .unwrap_or(false)
}

#[cfg(target_os = "windows")]
fn lookup_in_path(cmd: &str) -> Option<String> {
    if cmd.contains('\\') || cmd.contains('/') {
        let p = std::path::Path::new(cmd);
        return (p.is_file() && !is_reparse_point(p)).then(|| p.to_string_lossy().to_string());
    }
    let path_var = std::env::var("PATH").unwrap_or_default();
    let pathext = std::env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".to_string());
    for dir in path_var.split(';').filter(|s| !s.is_empty()) {
        let bare = std::path::Path::new(dir).join(cmd);
        if bare.is_file() && !is_reparse_point(&bare) {
            return Some(bare.to_string_lossy().to_string());
        }
        for ext in pathext.split(';').filter(|s| !s.is_empty()) {
            let candidate = std::path::Path::new(dir).join(format!("{cmd}{ext}"));
            if candidate.is_file() && !is_reparse_point(&candidate) {
                return Some(candidate.to_string_lossy().to_string());
            }
        }
    }
    None
}

#[cfg(not(target_os = "windows"))]
fn lookup_in_path(cmd: &str) -> Option<String> {
    use std::os::unix::fs::PermissionsExt;
    if cmd.contains('/') {
        let p = std::path::Path::new(cmd);
        return p.is_file().then(|| p.to_string_lossy().to_string());
    }
    for dir in std::env::var("PATH").unwrap_or_default().split(':').filter(|s| !s.is_empty()) {
        let candidate = std::path::Path::new(dir).join(cmd);
        if let Ok(meta) = std::fs::metadata(&candidate) {
            if meta.is_file() && meta.permissions().mode() & 0o111 != 0 {
                return Some(candidate.to_string_lossy().to_string());
            }
        }
    }
    None
}

fn run_version(cmd: &str, args: &[&str]) -> Option<String> {
    use std::sync::mpsc;
    let cmd = cmd.to_string();
    let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut command = Command::new(&cmd);
        command.args(&args);
        #[cfg(target_os = "windows")]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(CREATE_NO_WINDOW);
        }
        let result = command
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| {
                let out = String::from_utf8_lossy(&o.stdout).trim().to_string();
                if out.is_empty() {
                    String::from_utf8_lossy(&o.stderr).trim().to_string()
                } else {
                    out
                }
            })
            .filter(|s| !s.is_empty());
        let _ = tx.send(result);
    });
    rx.recv_timeout(std::time::Duration::from_secs(5)).ok().flatten()
}

fn parse_node_version(s: &str) -> Option<(u32, u32)> {
    let s = s.trim().trim_start_matches('v');
    let mut parts = s.split('.');
    Some((parts.next()?.parse().ok()?, parts.next()?.parse().ok()?))
}

fn check_tool(id: &str, name: &str, cmd: &str) -> ToolStatus {
    let path = lookup_in_path(cmd);
    let version = path
        .as_deref()
        .and_then(|p| run_version(p, &["--version"]))
        .or_else(|| run_version(cmd, &["--version"]));
    let installed = path.is_some();
    let needs_upgrade = id == "node"
        && installed
        && version
            .as_deref()
            .and_then(parse_node_version)
            .map(|v| v < MIN_NODE_FOR_CLI)
            .unwrap_or(false);
    write_log(&format!(
        "check_tool({id}): path={path:?}, version={version:?}, installed={installed}, needs_upgrade={needs_upgrade}"
    ));
    ToolStatus {
        id: id.to_string(),
        name: name.to_string(),
        installed,
        version,
        path,
        installable: true,
        needs_upgrade,
    }
}

#[tauri::command]
pub async fn check_tools_installed() -> Result<HomeToolsStatus, String> {
    tokio::task::spawn_blocking(|| {
        prepare_path();
        write_log("========== check_tools_installed ==========");
        [
            ("node", "Node.js", "node"),
            ("git", "Git", "git"),
            ("claude", "Claude Code", "claude"),
            ("codex", "Codex", "codex"),
            ("gemini", "Gemini CLI", "gemini"),
        ]
        .into_iter()
        .map(|(id, name, cmd)| check_tool(id, name, cmd))
        .collect::<Vec<_>>()
    })
    .await
    .map(|tools| HomeToolsStatus { tools })
    .map_err(|e| format!("Failed to check tools: {e}"))
}

#[cfg(target_os = "windows")]
fn install_command(tool_id: &str) -> Result<(String, Vec<String>, bool), String> {
    match tool_id {
        "node" => Ok((
            "winget".into(),
            vec![
                "install".into(),
                "--id".into(),
                "OpenJS.NodeJS.LTS".into(),
                "-e".into(),
                "--silent".into(),
                "--accept-source-agreements".into(),
                "--accept-package-agreements".into(),
            ],
            false,
        )),
        "git" => Ok((
            "winget".into(),
            vec![
                "install".into(),
                "--id".into(),
                "Git.Git".into(),
                "-e".into(),
                "--silent".into(),
                "--accept-source-agreements".into(),
                "--accept-package-agreements".into(),
            ],
            false,
        )),
        "claude" => Ok(("cmd".into(), vec!["/C".into(), "npm".into(), "install".into(), "-g".into(), "@anthropic-ai/claude-code".into(), "--registry".into(), "https://registry.npmmirror.com".into()], true)),
        "codex" => Ok(("cmd".into(), vec!["/C".into(), "npm".into(), "install".into(), "-g".into(), "@openai/codex".into(), "--registry".into(), "https://registry.npmmirror.com".into()], true)),
        "gemini" => Ok(("cmd".into(), vec!["/C".into(), "npm".into(), "install".into(), "-g".into(), "@google/gemini-cli".into(), "--registry".into(), "https://registry.npmmirror.com".into()], true)),
        other => Err(format!("Unknown tool id: {other}")),
    }
}

#[cfg(target_os = "macos")]
fn install_command(tool_id: &str) -> Result<(String, Vec<String>, bool), String> {
    match tool_id {
        "node" => Ok(("brew".into(), vec!["install".into(), "node".into()], false)),
        "git" => Ok(("brew".into(), vec!["install".into(), "git".into()], false)),
        "claude" => Ok(("npm".into(), vec!["install".into(), "-g".into(), "@anthropic-ai/claude-code".into(), "--registry".into(), "https://registry.npmmirror.com".into()], true)),
        "codex" => Ok(("npm".into(), vec!["install".into(), "-g".into(), "@openai/codex".into(), "--registry".into(), "https://registry.npmmirror.com".into()], true)),
        "gemini" => Ok(("npm".into(), vec!["install".into(), "-g".into(), "@google/gemini-cli".into(), "--registry".into(), "https://registry.npmmirror.com".into()], true)),
        other => Err(format!("Unknown tool id: {other}")),
    }
}

#[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
fn install_command(tool_id: &str) -> Result<(String, Vec<String>, bool), String> {
    match tool_id {
        "node" => Ok(("sh".into(), vec!["-c".into(), "curl -fsSL https://deb.nodesource.com/setup_lts.x | sudo -E bash - && sudo apt-get install -y nodejs".into()], false)),
        "git" => Ok(("sh".into(), vec!["-c".into(), "sudo apt-get install -y git || sudo dnf install -y git".into()], false)),
        "claude" => Ok(("npm".into(), vec!["install".into(), "-g".into(), "@anthropic-ai/claude-code".into(), "--registry".into(), "https://registry.npmmirror.com".into()], true)),
        "codex" => Ok(("npm".into(), vec!["install".into(), "-g".into(), "@openai/codex".into(), "--registry".into(), "https://registry.npmmirror.com".into()], true)),
        "gemini" => Ok(("npm".into(), vec!["install".into(), "-g".into(), "@google/gemini-cli".into(), "--registry".into(), "https://registry.npmmirror.com".into()], true)),
        other => Err(format!("Unknown tool id: {other}")),
    }
}

#[tauri::command]
pub async fn install_tool(tool_id: String) -> Result<InstallResult, String> {
    tokio::task::spawn_blocking(move || {
        prepare_path();
        let log_file = frogclaw_log_path().map(|p| p.to_string_lossy().to_string());
        write_log(&format!("========== Installing tool: {tool_id} =========="));

        let (program, args, requires_node) = install_command(&tool_id)?;
        if requires_node && lookup_in_path("node").is_none() {
            return Ok(InstallResult {
                success: false,
                stdout: String::new(),
                stderr: String::new(),
                message: "需要先安装 Node.js 才能安装该工具".into(),
                log_file,
            });
        }

        let spawn_target = lookup_in_path(&program).unwrap_or(program.clone());
        write_log(&format!("Command: {} {}", spawn_target, args.join(" ")));
        let mut command = Command::new(spawn_target);
        command.args(&args);
        #[cfg(target_os = "windows")]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(CREATE_NO_WINDOW);
        }

        match command.output() {
            Ok(out) => {
                let stdout = String::from_utf8_lossy(&out.stdout).to_string();
                let stderr = String::from_utf8_lossy(&out.stderr).to_string();
                let success = out.status.success();
                if !stdout.trim().is_empty() {
                    write_log(&format!("STDOUT:\n{}", stdout.trim()));
                }
                if !stderr.trim().is_empty() {
                    write_log(&format!("STDERR:\n{}", stderr.trim()));
                }
                let message = if success {
                    format!("{tool_id} 安装成功")
                } else {
                    format!(
                        "{tool_id} 安装失败 (exit {})",
                        out.status.code().unwrap_or(-1)
                    )
                };
                Ok(InstallResult {
                    success,
                    stdout,
                    stderr,
                    message,
                    log_file,
                })
            }
            Err(e) => Ok(InstallResult {
                success: false,
                stdout: String::new(),
                stderr: String::new(),
                message: format!("执行安装命令失败: {e}"),
                log_file,
            }),
        }
    })
    .await
    .map_err(|e| format!("Failed to spawn install task: {e}"))?
}
