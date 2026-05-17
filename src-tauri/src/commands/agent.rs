use crate::AppState;
use frogclaw_core::repo::{agent_session, conversation, message, provider};
use frogclaw_core::token_counter;
use frogclaw_core::types::{
    AgentEngineInfo, AgentSession, MessageRole, ProviderConfig, ProviderProxyConfig,
    RagContextResult,
};
use frogclaw_providers::{resolve_base_url_for_type, ProviderRequestContext};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::process::{Command as StdCommand, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use tauri::{Emitter, State};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{oneshot, RwLock};

#[cfg(windows)]
use std::os::windows::process::CommandExt;

/// In-memory map of conversation IDs to actively running agent task IDs.
/// Used as the source of truth for concurrency checks (more reliable than DB status).
static RUNNING_AGENTS: LazyLock<Mutex<HashMap<String, String>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

const ENGINE_CODEX_APP_SERVER: &str = "codex_app_server";
const ENGINE_FROG_AGENT: &str = "frog_agent";
const ENGINE_CLAUDE_CODE: &str = "claude_code";
const ENGINE_CODEX_CLI: &str = "codex_cli";
const ENGINE_GEMINI_CLI: &str = "gemini_cli";

#[derive(Clone, Default)]
pub struct AgentCancellationToken {
    cancelled: Arc<AtomicBool>,
}

impl AgentCancellationToken {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }
}

/// RAII guard that removes a conversation ID from RUNNING_AGENTS on drop.
/// Ensures cleanup even if the spawned task panics.
struct RunningAgentGuard {
    conversation_id: String,
    run_id: String,
}

impl Drop for RunningAgentGuard {
    fn drop(&mut self) {
        if let Ok(mut running) = RUNNING_AGENTS.lock() {
            if running.get(&self.conversation_id) == Some(&self.run_id) {
                running.remove(&self.conversation_id);
            }
        }
    }
}

struct AgentCancelTokenGuard {
    conversation_id: String,
    tokens: Arc<tokio::sync::Mutex<HashMap<String, AgentCancellationToken>>>,
}

impl Drop for AgentCancelTokenGuard {
    fn drop(&mut self) {
        let conversation_id = self.conversation_id.clone();
        let tokens = self.tokens.clone();
        tokio::spawn(async move {
            tokens.lock().await.remove(&conversation_id);
        });
    }
}

async fn ensure_agent_assistant_message(
    db: &sea_orm::DatabaseConnection,
    app: &tauri::AppHandle,
    conv_id: &str,
    user_msg_id: &str,
    content: &str,
    current_assistant_msg_id: &mut Option<String>,
    assistant_id_for_task: &Arc<RwLock<Option<String>>>,
) -> Option<String> {
    if let Some(message_id) = current_assistant_msg_id.clone() {
        return Some(message_id);
    }

    match message::create_message(
        db,
        conv_id,
        MessageRole::Assistant,
        content,
        &[],
        Some(user_msg_id),
        0,
    )
    .await
    {
        Ok(assist_msg) => {
            let message_id = assist_msg.id.clone();
            *current_assistant_msg_id = Some(message_id.clone());
            *assistant_id_for_task.write().await = Some(message_id.clone());
            tracing::info!("[agent] Created assistant message: {}", message_id);
            let _ = app.emit(
                "agent-message-id",
                serde_json::json!({
                    "conversationId": conv_id,
                    "assistantMessageId": message_id.clone(),
                }),
            );
            let _ = conversation::increment_message_count(db, conv_id).await;
            Some(message_id)
        }
        Err(err) => {
            tracing::warn!("[agent] Failed to create assistant message: {}", err);
            None
        }
    }
}

async fn persist_agent_partial_content(
    db: &sea_orm::DatabaseConnection,
    app: &tauri::AppHandle,
    conv_id: &str,
    user_msg_id: &str,
    content: &str,
    current_assistant_msg_id: &mut Option<String>,
    assistant_id_for_task: &Arc<RwLock<Option<String>>>,
) -> Option<String> {
    let message_id = ensure_agent_assistant_message(
        db,
        app,
        conv_id,
        user_msg_id,
        content,
        current_assistant_msg_id,
        assistant_id_for_task,
    )
    .await?;
    let _ = message::update_message_content(db, &message_id, content).await;
    Some(message_id)
}

// ---------------------------------------------------------------------------
// Payload types for Tauri events
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentDonePayload {
    #[serde(rename = "conversationId")]
    pub conversation_id: String,
    #[serde(rename = "assistantMessageId")]
    pub assistant_message_id: String,
    pub text: String,
    pub usage: Option<AgentUsagePayload>,
    #[serde(rename = "numTurns")]
    pub num_turns: Option<u32>,
    #[serde(rename = "costUsd")]
    pub cost_usd: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentUsagePayload {
    pub input_tokens: u64,
    pub output_tokens: u64,
}

#[derive(Debug, Clone)]
struct CodexUsage {
    input_tokens: u64,
    output_tokens: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentErrorPayload {
    #[serde(rename = "conversationId")]
    pub conversation_id: String,
    #[serde(rename = "assistantMessageId")]
    pub assistant_message_id: Option<String>,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentToolStartPayload {
    #[serde(rename = "conversationId")]
    pub conversation_id: String,
    #[serde(rename = "assistantMessageId")]
    pub assistant_message_id: String,
    #[serde(rename = "toolUseId")]
    pub tool_use_id: String,
    #[serde(rename = "toolName")]
    pub tool_name: String,
    pub input: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentToolUsePayload {
    #[serde(rename = "conversationId")]
    pub conversation_id: String,
    #[serde(rename = "assistantMessageId")]
    pub assistant_message_id: String,
    #[serde(rename = "toolUseId")]
    pub tool_use_id: String,
    #[serde(rename = "toolName")]
    pub tool_name: String,
    pub input: Value,
    #[serde(rename = "executionId")]
    pub execution_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentToolResultPayload {
    #[serde(rename = "conversationId")]
    pub conversation_id: String,
    #[serde(rename = "assistantMessageId")]
    pub assistant_message_id: String,
    #[serde(rename = "toolUseId")]
    pub tool_use_id: String,
    #[serde(rename = "toolName")]
    pub tool_name: String,
    pub content: String,
    #[serde(rename = "isError")]
    pub is_error: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentPermissionRequestPayload {
    #[serde(rename = "conversationId")]
    pub conversation_id: String,
    #[serde(rename = "assistantMessageId")]
    pub assistant_message_id: String,
    #[serde(rename = "toolUseId")]
    pub tool_use_id: String,
    #[serde(rename = "toolName")]
    pub tool_name: String,
    pub input: Value,
    #[serde(rename = "riskLevel")]
    pub risk_level: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentStatusPayload {
    #[serde(rename = "conversationId")]
    pub conversation_id: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentTextPayload {
    #[serde(rename = "conversationId")]
    pub conversation_id: String,
    #[serde(rename = "assistantMessageId")]
    pub assistant_message_id: String,
    pub text: String,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn is_supported_engine_kind(kind: &str) -> bool {
    matches!(
        kind,
        ENGINE_CODEX_APP_SERVER
            | ENGINE_FROG_AGENT
            | ENGINE_CLAUDE_CODE
            | ENGINE_CODEX_CLI
            | ENGINE_GEMINI_CLI
    )
}

fn escape_toml_string(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
}

fn codex_sandbox_mode(permission_mode: &str) -> &'static str {
    match permission_mode {
        "full_access" => "danger-full-access",
        "accept_edits" => "workspace-write",
        _ => "workspace-write",
    }
}

fn codex_approval_policy(permission_mode: &str) -> &'static str {
    match permission_mode {
        "full_access" => "never",
        "accept_edits" => "on-request",
        _ => "on-request",
    }
}

struct CodexRuntimeConfig {
    codex_home: PathBuf,
    env_key_name: String,
    api_key: String,
}

fn write_codex_config(
    model_id: &str,
    base_url: &str,
    api_key: String,
    permission_mode: &str,
) -> Result<CodexRuntimeConfig, String> {
    let codex_home = crate::paths::frogclaw_home().join("codex");
    let state_dir = codex_home.join("state");
    let log_dir = codex_home.join("log");
    fs::create_dir_all(&state_dir)
        .map_err(|e| format!("Failed to create Codex state directory: {e}"))?;
    fs::create_dir_all(&log_dir)
        .map_err(|e| format!("Failed to create Codex log directory: {e}"))?;

    let env_key_name = "FROG_CODEX_API_KEY".to_string();
    let config = format!(
        "model = \"{}\"\nmodel_provider = \"frog-provider\"\nsandbox_mode = \"{}\"\napproval_policy = \"{}\"\nsqlite_home = \"{}\"\nlog_dir = \"{}\"\n\n[model_providers.frog-provider]\nname = \"Frog Provider\"\nbase_url = \"{}\"\nenv_key = \"{}\"\nwire_api = \"responses\"\nrequires_openai_auth = false\n",
        escape_toml_string(model_id),
        codex_sandbox_mode(permission_mode),
        codex_approval_policy(permission_mode),
        escape_toml_string(&state_dir.to_string_lossy().replace('\\', "/")),
        escape_toml_string(&log_dir.to_string_lossy().replace('\\', "/")),
        escape_toml_string(base_url.trim_end_matches('/')),
        env_key_name,
    );

    let config_path = codex_home.join("config.toml");
    let tmp_path = codex_home.join("config.toml.tmp");
    fs::write(&tmp_path, config).map_err(|e| format!("Failed to write Codex config: {e}"))?;
    fs::rename(&tmp_path, &config_path)
        .map_err(|e| format!("Failed to replace Codex config: {e}"))?;

    Ok(CodexRuntimeConfig {
        codex_home,
        env_key_name,
        api_key,
    })
}

fn codex_app_server_log_path() -> PathBuf {
    crate::paths::frogclaw_home().join("ai-agent.log")
}

pub fn init_ai_agent_log_file() {
    let path = codex_app_server_log_path();
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if path.exists() {
        let archive_stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
        let archive_path = path.with_file_name(format!("ai-agent.{archive_stamp}.log"));
        let _ = fs::rename(&path, archive_path);
    }
    let timestamp = chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f");
    let _ = fs::write(path, format!("[{timestamp}] AI Agent log started\n"));
}

fn strip_ansi_control_sequences(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut chars = value.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\u{1b}' {
            if chars.peek() == Some(&'[') {
                let _ = chars.next();
                for next in chars.by_ref() {
                    if ('@'..='~').contains(&next) {
                        break;
                    }
                }
            }
            continue;
        }
        out.push(ch);
    }
    out
}

fn compact_codex_log_message(message: &str) -> String {
    let cleaned = strip_ansi_control_sequences(message)
        .replace('\r', "\\r")
        .replace('\n', "\\n");
    const MAX_LEN: usize = 600;
    if cleaned.chars().count() <= MAX_LEN {
        cleaned
    } else {
        format!(
            "{}... <truncated>",
            cleaned.chars().take(MAX_LEN).collect::<String>()
        )
    }
}

fn append_codex_app_server_log(message: impl AsRef<str>) {
    let path = codex_app_server_log_path();
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    const MAX_LOG_BYTES: u64 = 1024 * 1024;
    if fs::metadata(&path)
        .map(|metadata| metadata.len() > MAX_LOG_BYTES)
        .unwrap_or(false)
    {
        let rotated = path.with_extension("log.1");
        let _ = fs::remove_file(&rotated);
        let _ = fs::rename(&path, rotated);
    }
    let timestamp = chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f");
    let line = compact_codex_log_message(message.as_ref());
    if let Ok(mut file) = fs::OpenOptions::new().create(true).append(true).open(path) {
        use std::io::Write;
        let _ = writeln!(file, "[{timestamp}] {line}");
    }
}

fn append_codex_app_server_event_log(method: &str, params: &Value) {
    match method {
        "thread/tokenUsage/updated" => {
            if let Some(usage) = extract_codex_usage(params) {
                append_codex_app_server_log(format!(
                    "token usage input={} output={}",
                    usage.input_tokens, usage.output_tokens
                ));
            }
        }
        "turn/completed" => append_codex_app_server_log("turn completed"),
        "item/started" => {
            if let Some((_id, tool_name, input)) = extract_app_server_tool_info(params) {
                append_codex_app_server_log(format!(
                    "tool started {tool_name}: {}",
                    get_tool_input_summary(&tool_name, &input)
                ));
            }
        }
        "item/completed" => {
            if let Some((_id, tool_name, input)) = extract_app_server_tool_info(params) {
                append_codex_app_server_log(format!(
                    "tool completed {tool_name}: {}",
                    get_tool_input_summary(&tool_name, &input)
                ));
            }
        }
        method if method.contains("requestApproval") || method.ends_with("Approval") => {
            append_codex_app_server_log(format!("approval request {method}"));
        }
        method if method.contains("error") || method.contains("failed") => {
            append_codex_app_server_log(format!("event {method}: {params}"));
        }
        _ => {}
    }
}

fn path_entries() -> Vec<PathBuf> {
    std::env::var_os("PATH")
        .map(|paths| std::env::split_paths(&paths).collect())
        .unwrap_or_default()
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
}

fn candidate_binary_names(base: &str) -> Vec<String> {
    if cfg!(windows) {
        vec![
            format!("{base}.exe"),
            format!("{base}.cmd"),
            format!("{base}.bat"),
            base.to_string(),
        ]
    } else {
        vec![base.to_string()]
    }
}

fn find_binary(base: &str, extra_dirs: &[PathBuf]) -> Option<PathBuf> {
    let names = candidate_binary_names(base);
    let mut dirs = Vec::new();
    dirs.extend(extra_dirs.iter().cloned());
    dirs.extend(path_entries());

    for dir in dirs {
        for name in &names {
            let path = dir.join(name);
            if path.is_file() {
                return Some(path);
            }
        }
    }
    None
}

fn claude_candidate_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(home) = home_dir() {
        dirs.push(home.join(".local").join("bin"));
        dirs.push(home.join(".claude").join("bin"));
        dirs.push(home.join(".bun").join("bin"));
        dirs.push(home.join(".npm-global").join("bin"));
    }
    if cfg!(windows) {
        if let Some(appdata) = std::env::var_os("APPDATA") {
            dirs.push(PathBuf::from(appdata).join("npm"));
        }
        if let Some(localappdata) = std::env::var_os("LOCALAPPDATA") {
            dirs.push(PathBuf::from(localappdata).join("npm"));
        }
    } else {
        dirs.push(PathBuf::from("/opt/homebrew/bin"));
        dirs.push(PathBuf::from("/usr/local/bin"));
        dirs.push(PathBuf::from("/usr/bin"));
    }
    dirs
}

fn packaged_binary_candidate_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Ok(exe_path) = std::env::current_exe() {
        if let Some(exe_parent) = exe_path.parent() {
            dirs.push(exe_parent.to_path_buf());
            dirs.push(exe_parent.join("resources"));
            dirs.push(exe_parent.join("resources").join("binaries"));
            dirs.push(exe_parent.join("binaries"));
        }
    }
    dirs
}

fn codex_rs_root_from_env() -> Option<PathBuf> {
    let root = PathBuf::from(std::env::var_os("FROG_CODEX_RS")?);
    if root.join("Cargo.toml").is_file() {
        return Some(root);
    }
    let nested = root.join("codex-rs");
    if nested.join("Cargo.toml").is_file() {
        return Some(nested);
    }
    Some(root)
}

fn codex_app_server_candidate_dirs() -> Vec<PathBuf> {
    let mut dirs = packaged_binary_candidate_dirs();

    #[cfg(windows)]
    {
        dirs.extend(claude_candidate_dirs());
    }

    if let Some(root) = codex_rs_root_from_env() {
        dirs.push(root.join("target").join("release"));
        dirs.push(root.join("target").join("debug"));
    }

    #[cfg(windows)]
    {
        let local_codex_rs = PathBuf::from(r"E:\frogclaw\codex\codex-rs");
        dirs.push(local_codex_rs.join("target").join("release"));
        dirs.push(local_codex_rs.join("target").join("debug"));
    }

    dirs
}

fn command_version(path: &PathBuf) -> Option<String> {
    let mut cmd = StdCommand::new(path);
    cmd.arg("--version");
    #[cfg(windows)]
    {
        cmd.creation_flags(0x08000000);
    }
    cmd.output().ok().and_then(|output| {
        let text = if output.stdout.is_empty() {
            String::from_utf8_lossy(&output.stderr).trim().to_string()
        } else {
            String::from_utf8_lossy(&output.stdout).trim().to_string()
        };
        if text.is_empty() {
            None
        } else {
            Some(text.lines().next().unwrap_or_default().to_string())
        }
    })
}

fn cli_engine_info(
    kind: &str,
    display_name: &str,
    description: &str,
    binary_name: &str,
    extra_dirs: &[PathBuf],
    experimental: bool,
) -> AgentEngineInfo {
    let binary_path = find_binary(binary_name, extra_dirs);
    let version = binary_path.as_ref().and_then(command_version);
    let installed = binary_path.is_some();
    AgentEngineInfo {
        kind: kind.to_string(),
        display_name: display_name.to_string(),
        description: description.to_string(),
        available: installed && !experimental,
        installed,
        version,
        binary_path: binary_path
            .as_ref()
            .map(|path| path.to_string_lossy().to_string()),
        status: if installed {
            if experimental {
                "experimental".to_string()
            } else {
                "available".to_string()
            }
        } else {
            "not_installed".to_string()
        },
        message: if installed {
            None
        } else {
            Some(format!("{display_name} CLI not found"))
        },
        experimental,
    }
}

/// Extract a short human-readable summary from tool input JSON for inline rendering.
fn get_tool_input_summary(tool_name: &str, input: &Value) -> String {
    let try_key = |key: &str| {
        input
            .get(key)
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
    };

    if let Some(cmd) = try_key("command") {
        return cmd.chars().take(80).collect();
    }
    if let Some(path) = try_key("path").or_else(|| try_key("file_path")) {
        return path;
    }
    if let Some(pattern) = try_key("pattern") {
        return pattern.chars().take(80).collect();
    }
    if let Some(query) = try_key("query") {
        return query.chars().take(80).collect();
    }
    if let Some(content) = try_key("content") {
        return content.chars().take(60).collect();
    }
    // Fallback: first string value
    if let Some(obj) = input.as_object() {
        for val in obj.values() {
            if let Some(s) = val.as_str() {
                return s.chars().take(80).collect();
            }
        }
    }
    tool_name.to_string()
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

#[tauri::command]
pub async fn agent_query(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    conversation_id: String,
    prompt: String,
    provider_id: String,
    model_id: String,
) -> Result<(), String> {
    // 1. Get agent session (must exist)
    let session =
        agent_session::get_agent_session_by_conversation_id(&state.sea_db, &conversation_id)
            .await
            .map_err(|e| e.to_string())?
            .ok_or("Agent session not found. Please switch to Agent mode first.")?;

    // 2. Concurrent check — use in-memory set as source of truth
    {
        let running = RUNNING_AGENTS.lock().unwrap();
        if running.contains_key(&conversation_id) {
            return Err("Agent is already running".to_string());
        }
    }

    // 3. Set runtime_status to 'running'
    agent_session::update_agent_session_status(&state.sea_db, &session.id, "running")
        .await
        .map_err(|e| e.to_string())?;

    // 4. Save user message
    let user_message = message::create_message(
        &state.sea_db,
        &conversation_id,
        MessageRole::User,
        &prompt,
        &[],
        None,
        0,
    )
    .await
    .map_err(|e| e.to_string())?;

    // Check if first message BEFORE incrementing
    let pre_conv = conversation::get_conversation(&state.sea_db, &conversation_id)
        .await
        .map_err(|e| e.to_string())?;
    let is_first_message = pre_conv.message_count <= 1;

    conversation::increment_message_count(&state.sea_db, &conversation_id)
        .await
        .map_err(|e| e.to_string())?;

    // Auto-title: set fallback + async AI title for first message
    if is_first_message {
        let fallback_title = if prompt.chars().count() > 30 {
            format!("{}...", prompt.chars().take(30).collect::<String>())
        } else {
            prompt.clone()
        };
        if let Err(e) = conversation::update_conversation_title(
            &state.sea_db,
            &conversation_id,
            &fallback_title,
        )
        .await
        {
            tracing::error!("[agent] Failed to set fallback title: {}", e);
        } else {
            let _ = app.emit(
                "conversation-title-updated",
                frogclaw_core::types::ConversationTitleUpdatedEvent {
                    conversation_id: conversation_id.clone(),
                    title: fallback_title,
                },
            );
        }
    }

    if !matches!(
        session.engine_kind.as_str(),
        ENGINE_CLAUDE_CODE | ENGINE_CODEX_CLI | ENGINE_CODEX_APP_SERVER | ENGINE_FROG_AGENT
    ) {
        let _ =
            agent_session::update_agent_session_status(&state.sea_db, &session.id, "idle").await;
        return Err(format!(
            "Agent engine '{}' is registered but not implemented yet",
            session.engine_kind
        ));
    }

    let plan_mode = matches!(
        session.engine_kind.as_str(),
        ENGINE_CODEX_APP_SERVER | ENGINE_FROG_AGENT
    ) && prompt.trim_start().starts_with("/plan");
    let task_prompt = if plan_mode {
        prompt
            .trim_start()
            .strip_prefix("/plan")
            .unwrap_or(&prompt)
            .trim_start()
            .to_string()
    } else {
        prompt.clone()
    };
    let prompt_for_task = prepare_agent_prompt_with_memory(
        &state.sea_db,
        &conversation_id,
        session.cwd.as_deref(),
        &task_prompt,
    )
    .await;

    if matches!(
        session.engine_kind.as_str(),
        ENGINE_CLAUDE_CODE | ENGINE_CODEX_CLI
    ) {
        let db = state.sea_db.clone();
        let tokens = state.agent_cancel_tokens.clone();
        let session_id = session.id.clone();
        let user_msg_id = user_message.id.clone();
        let cwd = session.cwd.clone();
        let permission_mode = session.permission_mode.clone();
        let conv_id = conversation_id.clone();
        if session.engine_kind == ENGINE_CLAUDE_CODE {
            tokio::spawn(async move {
                run_claude_code_cli_query(
                    app,
                    db,
                    tokens,
                    conv_id,
                    session_id,
                    user_msg_id,
                    prompt_for_task,
                    cwd,
                    permission_mode,
                    is_first_message,
                )
                .await;
            });
        } else {
            tokio::spawn(async move {
                run_codex_cli_query(
                    app,
                    db,
                    tokens,
                    conv_id,
                    session_id,
                    user_msg_id,
                    prompt_for_task,
                    cwd,
                    permission_mode,
                    None,
                    is_first_message,
                )
                .await;
            });
        }
        return Ok(());
    }

    let db = state.sea_db.clone();
    let tokens = state.agent_cancel_tokens.clone();
    let permission_senders = state.agent_permission_senders.clone();
    let vector_store = state.vector_store.clone();
    let session_id = session.id.clone();
    let user_msg_id = user_message.id.clone();
    let cwd = session.cwd.clone();
    let permission_mode = session.permission_mode.clone();
    let conv_id = conversation_id.clone();
    let master_key = state.master_key;

    tokio::spawn(async move {
        run_codex_app_server_query(
            app,
            db,
            tokens,
            permission_senders,
            vector_store,
            conv_id,
            session_id,
            user_msg_id,
            prompt_for_task,
            task_prompt,
            provider_id,
            model_id,
            cwd,
            permission_mode,
            master_key,
            is_first_message,
            plan_mode,
        )
        .await;
    });

    Ok(())
}
#[tauri::command]
pub async fn agent_approve(
    state: State<'_, AppState>,
    _conversation_id: String,
    tool_use_id: String,
    decision: String,
) -> Result<(), String> {
    if !["allow_once", "allow_always", "deny"].contains(&decision.as_str()) {
        return Err(format!("Invalid decision: {}", decision));
    }

    // Look up the stored oneshot sender for this tool_use_id
    let sender = state
        .agent_permission_senders
        .lock()
        .await
        .remove(&tool_use_id);

    match sender {
        Some(tx) => {
            tx.send(decision)
                .map_err(|_| "Permission channel closed".to_string())?;
            Ok(())
        }
        None => Err(format!(
            "No pending permission request for tool_use_id: {}",
            tool_use_id
        )),
    }
}

#[tauri::command]
pub async fn agent_respond_ask(
    state: State<'_, AppState>,
    ask_id: String,
    answer: String,
) -> Result<(), String> {
    let sender = state.agent_ask_senders.lock().await.remove(&ask_id);

    match sender {
        Some(tx) => {
            tx.send(answer)
                .map_err(|_| "Ask user channel closed".to_string())?;
            Ok(())
        }
        None => Err(format!("No pending ask request for ask_id: {}", ask_id)),
    }
}

#[tauri::command]
pub async fn agent_cancel(
    state: State<'_, AppState>,
    conversation_id: String,
) -> Result<(), String> {
    let session =
        agent_session::get_agent_session_by_conversation_id(&state.sea_db, &conversation_id)
            .await
            .map_err(|e| e.to_string())?
            .ok_or("Agent session not found")?;

    // Reset DB status to idle
    agent_session::update_agent_session_status(&state.sea_db, &session.id, "idle")
        .await
        .map_err(|e| e.to_string())?;

    if let Some(token) = state
        .agent_cancel_tokens
        .lock()
        .await
        .remove(&conversation_id)
    {
        token.cancel();
    }

    // Remove from in-memory running set
    if let Ok(mut running) = RUNNING_AGENTS.lock() {
        running.remove(&conversation_id);
    }

    Ok(())
}

#[tauri::command]
pub async fn agent_update_session(
    state: State<'_, AppState>,
    conversation_id: String,
    cwd: Option<String>,
    permission_mode: Option<String>,
    engine_kind: Option<String>,
) -> Result<AgentSession, String> {
    if let Some(engine) = engine_kind.as_deref() {
        if !is_supported_engine_kind(engine) {
            return Err(format!("Unsupported agent engine: {engine}"));
        }
    }

    agent_session::upsert_agent_session(
        &state.sea_db,
        &conversation_id,
        cwd.as_deref(),
        permission_mode.as_deref(),
        engine_kind.as_deref(),
    )
    .await
    .map_err(|e| e.to_string())
}

fn extract_claude_texts(value: &Value) -> Vec<String> {
    let mut texts = Vec::new();
    if let Some(content) = value.pointer("/message/content").and_then(|v| v.as_array()) {
        for block in content {
            if block.get("type").and_then(|v| v.as_str()) == Some("text") {
                if let Some(text) = block.get("text").and_then(|v| v.as_str()) {
                    if !text.trim().is_empty() {
                        texts.push(text.to_string());
                    }
                }
            }
        }
    }
    texts
}

fn extract_codex_texts(value: &Value) -> Vec<String> {
    let mut texts = Vec::new();
    if value.get("type").and_then(|v| v.as_str()) == Some("item.completed") {
        if let Some(item) = value.get("item") {
            if item.get("type").and_then(|v| v.as_str()) == Some("agent_message") {
                if let Some(text) = item.get("text").and_then(|v| v.as_str()) {
                    if !text.trim().is_empty() {
                        texts.push(text.to_string());
                    }
                }
            }
        }
    }
    texts
}

fn extract_codex_usage(value: &Value) -> Option<CodexUsage> {
    let usage = value
        .get("usage")
        .or_else(|| value.get("params").and_then(|params| params.get("usage")))?;
    if value.get("type").and_then(|v| v.as_str()).is_some()
        && value.get("type").and_then(|v| v.as_str()) != Some("turn.completed")
    {
        return None;
    }
    Some(CodexUsage {
        input_tokens: usage
            .get("input_tokens")
            .or_else(|| usage.get("inputTokens"))
            .and_then(|v| v.as_u64())
            .unwrap_or_default(),
        output_tokens: usage
            .get("output_tokens")
            .or_else(|| usage.get("outputTokens"))
            .and_then(|v| v.as_u64())
            .unwrap_or_default(),
    })
}

fn estimate_agent_usage(prompt: &str, response: &str) -> CodexUsage {
    CodexUsage {
        input_tokens: token_counter::estimate_message_tokens("user", prompt) as u64,
        output_tokens: token_counter::estimate_message_tokens("assistant", response) as u64,
    }
}

fn effective_agent_usage(
    final_usage: Option<&CodexUsage>,
    prompt: &str,
    response: &str,
) -> CodexUsage {
    match final_usage {
        Some(usage) if usage.input_tokens > 0 || usage.output_tokens > 0 => usage.clone(),
        _ => estimate_agent_usage(prompt, response),
    }
}

fn project_name_from_path(project_path: &str) -> Option<String> {
    std::path::Path::new(project_path)
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.trim().is_empty())
        .map(ToString::to_string)
}

fn merge_agent_prompt_with_memory(prompt: &str, rag_result: &RagContextResult) -> String {
    if rag_result.context_parts.is_empty() {
        return prompt.to_string();
    }

    format!(
        "以下是 FrogClaw 为当前项目召回的长期记忆。请只在相关时使用，不要向用户暴露这段包装文本；如果记忆与用户请求冲突，以用户最新指令和仓库实际状态为准。\n\n{}\n\n用户请求：\n{}",
        rag_result.context_parts.join("\n\n"),
        prompt
    )
}

async fn prepare_agent_prompt_with_memory(
    db: &sea_orm::DatabaseConnection,
    conversation_id: &str,
    session_cwd: Option<&str>,
    prompt: &str,
) -> String {
    let conversation = match conversation::get_conversation(db, conversation_id).await {
        Ok(conversation) => conversation,
        Err(err) => {
            crate::claude_mem::append_memory_log(format!(
                "Agent 记忆召回：跳过 会话={} 原因=读取会话失败 错误={}",
                conversation_id,
                err.to_string().chars().take(240).collect::<String>()
            ));
            return prompt.to_string();
        }
    };

    let project_path = conversation
        .working_directory
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| session_cwd.filter(|value| !value.trim().is_empty()));
    let project_name = conversation
        .project_name
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .map(ToString::to_string)
        .or_else(|| project_path.and_then(project_name_from_path));

    if project_path.is_none() && project_name.is_none() {
        crate::claude_mem::append_memory_log(format!(
            "项目记忆召回：跳过 原因=没有项目目录 会话={}",
            conversation_id
        ));
        return prompt.to_string();
    }

    let project_label = project_name.as_deref().or(project_path).unwrap_or("-");
    crate::claude_mem::append_memory_log(format!(
        "项目记忆召回：开始 项目={} 项目路径={} 查询字符数={} top_k=5 会话={}",
        project_label,
        project_path.unwrap_or("-"),
        prompt.chars().count(),
        conversation_id
    ));

    match crate::claude_mem::collect_project_context(
        prompt,
        project_path,
        project_name.as_deref(),
        5,
    )
    .await
    {
        Ok(rag_result) => {
            crate::claude_mem::append_memory_log(format!(
                "项目记忆召回：完成 项目={} 来源数={} 上下文段数={} 会话={}",
                project_label,
                rag_result.source_results.len(),
                rag_result.context_parts.len(),
                conversation_id
            ));
            merge_agent_prompt_with_memory(prompt, &rag_result)
        }
        Err(err) => {
            crate::claude_mem::append_memory_log(format!(
                "项目记忆召回：失败 项目={} 错误={} 会话={}",
                project_label,
                err.chars().take(240).collect::<String>(),
                conversation_id
            ));
            prompt.to_string()
        }
    }
}

async fn write_app_server_message(
    stdin: &mut tokio::process::ChildStdin,
    value: &Value,
) -> Result<(), String> {
    let payload = serde_json::to_vec(value)
        .map_err(|err| format!("Failed to serialize Codex app-server JSON-RPC: {err}"))?;
    stdin
        .write_all(&payload)
        .await
        .map_err(|err| format!("Failed to write Codex app-server request: {err}"))?;
    stdin
        .write_all(b"\n")
        .await
        .map_err(|err| format!("Failed to write Codex app-server request terminator: {err}"))?;
    stdin
        .flush()
        .await
        .map_err(|err| format!("Failed to flush Codex app-server request: {err}"))?;
    Ok(())
}

async fn send_app_server_request(
    stdin: &mut tokio::process::ChildStdin,
    next_id: &mut i64,
    method: &str,
    params: Value,
) -> Result<i64, String> {
    let id = *next_id;
    *next_id += 1;
    write_app_server_message(
        stdin,
        &serde_json::json!({
            "id": id,
            "method": method,
            "params": params,
        }),
    )
    .await?;
    Ok(id)
}

async fn read_app_server_response<R>(
    lines: &mut tokio::io::Lines<BufReader<R>>,
    request_id: i64,
) -> Result<Value, String>
where
    R: tokio::io::AsyncRead + Unpin,
{
    loop {
        let line = lines
            .next_line()
            .await
            .map_err(|err| format!("Failed to read Codex app-server response: {err}"))?
            .ok_or_else(|| "Codex app-server closed stdout before responding".to_string())?;
        if line.trim().is_empty() {
            continue;
        }
        let value: Value = serde_json::from_str(&line)
            .map_err(|err| format!("Invalid Codex app-server JSON-RPC line: {err}: {line}"))?;
        if value.get("id").and_then(|v| v.as_i64()) != Some(request_id) {
            continue;
        }
        if let Some(error) = value.get("error") {
            let message = error
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown error");
            return Err(format!("Codex app-server request failed: {message}"));
        }
        return Ok(value.get("result").cloned().unwrap_or(Value::Null));
    }
}

fn extract_app_server_thread_id(result: &Value) -> Option<String> {
    result
        .get("thread")
        .and_then(|thread| thread.get("id"))
        .and_then(|id| id.as_str())
        .map(ToString::to_string)
}

fn extract_app_server_completed_text(params: &Value) -> Option<String> {
    let item = params.get("item")?;
    if item.get("type").and_then(|v| v.as_str()) != Some("agentMessage") {
        return None;
    }
    item.get("text")
        .and_then(|v| v.as_str())
        .filter(|text| !text.trim().is_empty())
        .map(ToString::to_string)
}

fn extract_app_server_completed_plan_text(params: &Value) -> Option<String> {
    let item = params.get("item")?;
    let item_type = item
        .get("type")
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    if !matches!(item_type, "plan" | "agentPlan" | "turnPlan") {
        return None;
    }
    item.get("text")
        .or_else(|| item.get("markdown"))
        .and_then(|v| v.as_str())
        .filter(|text| !text.trim().is_empty())
        .map(ToString::to_string)
}

fn extract_app_server_plan_delta(params: &Value) -> Option<String> {
    params
        .get("delta")
        .or_else(|| params.get("text"))
        .and_then(|v| v.as_str())
        .filter(|text| !text.is_empty())
        .map(ToString::to_string)
}

fn extract_app_server_plan_update(params: &Value) -> Option<String> {
    let plan = params.get("plan").unwrap_or(params);
    let mut lines = Vec::new();
    if let Some(explanation) = plan
        .get("explanation")
        .or_else(|| plan.get("summary"))
        .and_then(|v| v.as_str())
        .filter(|text| !text.trim().is_empty())
    {
        lines.push(explanation.trim().to_string());
    }
    if let Some(steps) = plan.get("steps").and_then(|v| v.as_array()) {
        for step in steps {
            let text = step
                .get("step")
                .or_else(|| step.get("text"))
                .or_else(|| step.get("description"))
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .trim();
            if text.is_empty() {
                continue;
            }
            let status = step
                .get("status")
                .and_then(|v| v.as_str())
                .unwrap_or("pending")
                .trim();
            lines.push(format!("- [{status}] {text}"));
        }
    }
    if lines.is_empty() {
        None
    } else {
        Some(lines.join("\n"))
    }
}

fn json_pick_string<'a>(value: &'a Value, keys: &[&str]) -> Option<&'a str> {
    for key in keys {
        if let Some(text) = value.get(*key).and_then(|v| v.as_str()) {
            if !text.trim().is_empty() {
                return Some(text);
            }
        }
    }
    None
}

fn extract_app_server_item(params: &Value) -> Option<&Value> {
    params.get("item").or_else(|| params.get("data"))
}

fn extract_app_server_tool_info(params: &Value) -> Option<(String, String, Value)> {
    let item = extract_app_server_item(params)?;
    let item_type = item
        .get("type")
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    let type_lower = item_type.to_lowercase();
    if !(type_lower.contains("command")
        || type_lower.contains("tool")
        || type_lower.contains("exec")
        || type_lower.contains("filechange"))
    {
        return None;
    }
    let id = json_pick_string(item, &["id", "itemId", "toolUseId", "callId"])
        .or_else(|| json_pick_string(params, &["id", "itemId", "toolUseId", "callId"]))?
        .to_string();
    let name = if type_lower.contains("filechange") {
        "file_change".to_string()
    } else if type_lower.contains("command") || type_lower.contains("exec") {
        "command".to_string()
    } else {
        json_pick_string(item, &["name", "toolName", "type"])
            .unwrap_or(item_type)
            .to_string()
    };
    let input = item
        .get("input")
        .or_else(|| item.get("params"))
        .or_else(|| item.get("arguments"))
        .cloned()
        .unwrap_or_else(|| item.clone());
    Some((id, name, input))
}

fn extract_app_server_output_delta(params: &Value) -> Option<(String, String)> {
    let id = json_pick_string(params, &["itemId", "id", "toolUseId", "callId"])?.to_string();
    let delta = params
        .get("delta")
        .or_else(|| params.get("text"))
        .or_else(|| params.get("output"))
        .and_then(|v| v.as_str())?
        .to_string();
    Some((id, delta))
}

fn html_attr_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn append_tool_call_tag(content: &mut String, tool_use_id: &str, tool_name: &str) {
    if !content.trim().is_empty() {
        content.push_str("\n\n");
    }
    content.push_str(&format!(
        "<tool-call data-frogclaw=\"1\" id=\"{}\" name=\"{}\"></tool-call>",
        html_attr_escape(tool_use_id),
        html_attr_escape(tool_name)
    ));
}

fn map_codex_permission_decision(method: &str, decision: &str) -> Value {
    let method_lower = method.to_lowercase();
    if matches!(method, "execCommandApproval" | "applyPatchApproval") {
        let mapped = match decision {
            "allow_always" => "approved_for_session",
            "allow_once" => "approved",
            _ => "denied",
        };
        return serde_json::json!({ "decision": mapped });
    }
    let mapped = match decision {
        "allow_always" => "acceptForSession",
        "allow_once" => "accept",
        _ => "decline",
    };
    if method_lower.contains("requestapproval") {
        serde_json::json!({ "decision": mapped })
    } else {
        serde_json::json!({ "decision": mapped })
    }
}

async fn request_codex_permission(
    app: &tauri::AppHandle,
    permission_senders: &Arc<tokio::sync::Mutex<HashMap<String, oneshot::Sender<String>>>>,
    db: &sea_orm::DatabaseConnection,
    conversation_id: &str,
    user_msg_id: &str,
    current_assistant_msg_id: &mut Option<String>,
    assistant_id_for_task: &Arc<RwLock<Option<String>>>,
    method: &str,
    request_id: i64,
    request_params: &Value,
) -> Result<Value, String> {
    let tool_use_id = json_pick_string(request_params, &["itemId", "id", "toolUseId", "callId"])
        .map(ToString::to_string)
        .or_else(|| {
            extract_app_server_item(request_params)
                .and_then(|item| json_pick_string(item, &["id", "itemId", "toolUseId", "callId"]))
                .map(ToString::to_string)
        })
        .unwrap_or_else(|| format!("codex-approval-{request_id}"));
    let method_lower = method.to_lowercase();
    let tool_name = if method_lower.contains("filechange") || method == "applyPatchApproval" {
        "file_change"
    } else {
        "command"
    };
    let assistant_message_id = ensure_agent_assistant_message(
        db,
        app,
        conversation_id,
        user_msg_id,
        "",
        current_assistant_msg_id,
        assistant_id_for_task,
    )
    .await
    .unwrap_or_default();

    let (tx, rx) = oneshot::channel();
    permission_senders
        .lock()
        .await
        .insert(tool_use_id.clone(), tx);
    let _ = app.emit(
        "agent-permission-request",
        AgentPermissionRequestPayload {
            conversation_id: conversation_id.to_string(),
            assistant_message_id: assistant_message_id.clone(),
            tool_use_id: tool_use_id.clone(),
            tool_name: tool_name.to_string(),
            input: request_params.clone(),
            risk_level: if tool_name == "file_change" {
                "write".to_string()
            } else {
                "execute".to_string()
            },
        },
    );
    let _ = app.emit(
        "agent-tool-use",
        AgentToolUsePayload {
            conversation_id: conversation_id.to_string(),
            assistant_message_id,
            tool_use_id: tool_use_id.clone(),
            tool_name: tool_name.to_string(),
            input: request_params.clone(),
            execution_id: None,
        },
    );

    let decision = rx
        .await
        .map_err(|_| "Permission response channel closed".to_string())?;
    Ok(map_codex_permission_decision(method, &decision))
}

async fn run_codex_app_server_query(
    app: tauri::AppHandle,
    db: sea_orm::DatabaseConnection,
    state_tokens: Arc<tokio::sync::Mutex<HashMap<String, AgentCancellationToken>>>,
    permission_senders: Arc<tokio::sync::Mutex<HashMap<String, oneshot::Sender<String>>>>,
    vector_store: Arc<frogclaw_core::vector_store::VectorStore>,
    conversation_id: String,
    session_id: String,
    user_msg_id: String,
    prompt: String,
    original_prompt: String,
    provider_id: String,
    model_id: String,
    cwd: Option<String>,
    permission_mode: String,
    master_key: [u8; 32],
    is_first_message: bool,
    plan_mode: bool,
) {
    let prov = match provider::get_provider(&db, &provider_id).await {
        Ok(prov) => prov,
        Err(err) => {
            let _ = app.emit(
                "agent-error",
                AgentErrorPayload {
                    conversation_id: conversation_id.clone(),
                    assistant_message_id: None,
                    message: format!("Failed to load provider for Codex runtime: {err}"),
                },
            );
            let _ = agent_session::update_agent_session_status(&db, &session_id, "idle").await;
            return;
        }
    };
    let key_row = match provider::get_active_key(&db, &provider_id).await {
        Ok(key) => key,
        Err(err) => {
            let _ = app.emit(
                "agent-error",
                AgentErrorPayload {
                    conversation_id: conversation_id.clone(),
                    assistant_message_id: None,
                    message: format!("Failed to load provider API key for Codex runtime: {err}"),
                },
            );
            let _ = agent_session::update_agent_session_status(&db, &session_id, "idle").await;
            return;
        }
    };
    let api_key = match frogclaw_core::crypto::decrypt_key(&key_row.key_encrypted, &master_key) {
        Ok(key) => key,
        Err(err) => {
            let _ = app.emit(
                "agent-error",
                AgentErrorPayload {
                    conversation_id: conversation_id.clone(),
                    assistant_message_id: None,
                    message: format!("Failed to decrypt provider API key for Codex runtime: {err}"),
                },
            );
            let _ = agent_session::update_agent_session_status(&db, &session_id, "idle").await;
            return;
        }
    };

    let base_url = resolve_base_url_for_type(&prov.api_host, &prov.provider_type);
    let settings = frogclaw_core::repo::settings::get_settings(&db)
        .await
        .unwrap_or_default();
    let proxy_config = ProviderProxyConfig::resolve(&prov.proxy_config, &settings);
    let ctx = ProviderRequestContext {
        api_key: api_key.clone(),
        key_id: key_row.id,
        provider_id: prov.id.clone(),
        base_url: Some(base_url.clone()),
        api_path: prov.api_path.clone(),
        proxy_config,
        custom_headers: prov
            .custom_headers
            .as_ref()
            .and_then(|s| serde_json::from_str(s).ok()),
    };
    let codex_config = match write_codex_config(&model_id, &base_url, api_key, &permission_mode) {
        Ok(config) => config,
        Err(err) => {
            let _ = app.emit(
                "agent-error",
                AgentErrorPayload {
                    conversation_id: conversation_id.clone(),
                    assistant_message_id: None,
                    message: err,
                },
            );
            let _ = agent_session::update_agent_session_status(&db, &session_id, "idle").await;
            return;
        }
    };

    append_codex_app_server_log(format!(
        "runtime config generated path={}",
        codex_config.codex_home.join("config.toml").display()
    ));

    run_codex_app_server_stdio_query(
        app,
        db,
        state_tokens,
        permission_senders,
        vector_store,
        conversation_id,
        session_id,
        user_msg_id,
        prompt,
        original_prompt,
        prov,
        ctx,
        codex_config,
        master_key,
        model_id,
        cwd,
        permission_mode,
        is_first_message,
        plan_mode,
    )
    .await;
}

async fn run_codex_app_server_stdio_query(
    app: tauri::AppHandle,
    db: sea_orm::DatabaseConnection,
    state_tokens: Arc<tokio::sync::Mutex<HashMap<String, AgentCancellationToken>>>,
    permission_senders: Arc<tokio::sync::Mutex<HashMap<String, oneshot::Sender<String>>>>,
    vector_store: Arc<frogclaw_core::vector_store::VectorStore>,
    conversation_id: String,
    session_id: String,
    user_msg_id: String,
    prompt: String,
    original_prompt: String,
    provider: ProviderConfig,
    provider_ctx: ProviderRequestContext,
    codex_config: CodexRuntimeConfig,
    master_key: [u8; 32],
    model_id: String,
    cwd: Option<String>,
    _permission_mode: String,
    is_first_message: bool,
    plan_mode: bool,
) {
    #[cfg(not(windows))]
    {
        let message = "Codex app-server is currently bundled for Windows builds only.".to_string();
        append_codex_app_server_log(&message);
        let _ = app.emit(
            "agent-error",
            AgentErrorPayload {
                conversation_id: conversation_id.clone(),
                assistant_message_id: None,
                message,
            },
        );
        let _ = agent_session::update_agent_session_status(&db, &session_id, "idle").await;
        return;
    }

    let run_id = format!("codex_app_server_{}", frogclaw_core::utils::gen_id());
    if let Ok(mut running) = RUNNING_AGENTS.lock() {
        running.insert(conversation_id.clone(), run_id.clone());
    }
    let _guard = RunningAgentGuard {
        conversation_id: conversation_id.clone(),
        run_id,
    };

    let cancel_token = AgentCancellationToken::new();
    state_tokens
        .lock()
        .await
        .insert(conversation_id.clone(), cancel_token.clone());
    let _cancel_guard = AgentCancelTokenGuard {
        conversation_id: conversation_id.clone(),
        tokens: state_tokens.clone(),
    };

    let app_server_path = match find_binary("codex-app-server", &codex_app_server_candidate_dirs())
    {
        Some(path) => path,
        None => {
            append_codex_app_server_log("binary not found");
            let _ = app.emit(
                "agent-error",
                AgentErrorPayload {
                    conversation_id: conversation_id.clone(),
                    assistant_message_id: None,
                    message: "Codex app-server binary not found. Build E:\\frogclaw\\codex\\codex-rs with `cargo build -p codex-app-server` or bundle codex-app-server.exe.".to_string(),
                },
            );
            let _ = agent_session::update_agent_session_status(&db, &session_id, "idle").await;
            return;
        }
    };

    let mut current_assistant_msg_id: Option<String> = None;
    let assistant_id_for_task: Arc<RwLock<Option<String>>> = Arc::new(RwLock::new(None));
    let mut accumulated_text = String::new();
    let mut saw_agent_delta = false;
    let mut plan_text = String::new();
    let mut tool_names: HashMap<String, String> = HashMap::new();
    let mut tool_outputs: HashMap<String, String> = HashMap::new();
    let mut final_usage: Option<CodexUsage> = None;

    let mut cmd = tokio::process::Command::new(&app_server_path);
    cmd.arg("--listen")
        .arg("stdio://")
        .current_dir(&codex_config.codex_home)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("CODEX_HOME", &codex_config.codex_home)
        .env("RUST_LOG", "warn")
        .env("CODEX_APP_SERVER_DISABLE_MANAGED_CONFIG", "1")
        .env(&codex_config.env_key_name, &codex_config.api_key);

    #[cfg(windows)]
    {
        cmd.creation_flags(0x08000000);
    }

    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(err) => {
            append_codex_app_server_log(format!(
                "spawn failed path={} cwd={} error={err}",
                app_server_path.display(),
                codex_config.codex_home.display()
            ));
            let _ = app.emit(
                "agent-error",
                AgentErrorPayload {
                    conversation_id: conversation_id.clone(),
                    assistant_message_id: None,
                    message: format!("Failed to start Codex app-server: {err}"),
                },
            );
            let _ = agent_session::update_agent_session_status(&db, &session_id, "idle").await;
            return;
        }
    };
    append_codex_app_server_log(format!(
        "spawned path={} cwd={} conversation_id={} session_id={}",
        app_server_path.display(),
        codex_config.codex_home.display(),
        conversation_id,
        session_id
    ));

    let Some(mut stdin) = child.stdin.take() else {
        let _ = app.emit(
            "agent-error",
            AgentErrorPayload {
                conversation_id: conversation_id.clone(),
                assistant_message_id: None,
                message: "Codex app-server stdin was not available".to_string(),
            },
        );
        let _ = agent_session::update_agent_session_status(&db, &session_id, "idle").await;
        return;
    };
    let Some(stdout) = child.stdout.take() else {
        let _ = app.emit(
            "agent-error",
            AgentErrorPayload {
                conversation_id: conversation_id.clone(),
                assistant_message_id: None,
                message: "Codex app-server stdout was not available".to_string(),
            },
        );
        let _ = agent_session::update_agent_session_status(&db, &session_id, "idle").await;
        return;
    };
    let stderr = child.stderr.take();
    let mut stdout_lines = BufReader::new(stdout).lines();
    let mut stderr_lines = stderr.map(|err| BufReader::new(err).lines());
    let mut next_request_id = 1_i64;

    append_codex_app_server_log("request initialize");

    let init_id = match send_app_server_request(
        &mut stdin,
        &mut next_request_id,
        "initialize",
        serde_json::json!({
            "clientInfo": {
                "name": "frogclaw",
                "title": "FrogClaw",
                "version": env!("CARGO_PKG_VERSION"),
            },
            "capabilities": {
                "experimentalApi": true,
            },
        }),
    )
    .await
    {
        Ok(id) => id,
        Err(err) => {
            append_codex_app_server_log(format!("initialize send failed: {err}"));
            let _ = app.emit(
                "agent-error",
                AgentErrorPayload {
                    conversation_id: conversation_id.clone(),
                    assistant_message_id: None,
                    message: err,
                },
            );
            let _ = child.kill().await;
            let _ = agent_session::update_agent_session_status(&db, &session_id, "idle").await;
            return;
        }
    };

    if let Err(err) = read_app_server_response(&mut stdout_lines, init_id).await {
        append_codex_app_server_log(format!("initialize failed: {err}"));
        let _ = app.emit(
            "agent-error",
            AgentErrorPayload {
                conversation_id: conversation_id.clone(),
                assistant_message_id: None,
                message: err,
            },
        );
        let _ = child.kill().await;
        let _ = agent_session::update_agent_session_status(&db, &session_id, "idle").await;
        return;
    }
    append_codex_app_server_log("response initialize ok");
    if let Err(err) = write_app_server_message(
        &mut stdin,
        &serde_json::json!({
            "method": "initialized",
        }),
    )
    .await
    {
        append_codex_app_server_log(format!("initialized notification failed: {err}"));
        let _ = app.emit(
            "agent-error",
            AgentErrorPayload {
                conversation_id: conversation_id.clone(),
                assistant_message_id: None,
                message: err,
            },
        );
        let _ = child.kill().await;
        let _ = agent_session::update_agent_session_status(&db, &session_id, "idle").await;
        return;
    }
    append_codex_app_server_log("notification initialized");

    append_codex_app_server_log(format!("request thread/start cwd={cwd:?}"));
    let thread_start_id = match send_app_server_request(
        &mut stdin,
        &mut next_request_id,
        "thread/start",
        serde_json::json!({
            "model": null,
            "modelProvider": null,
            "cwd": cwd.clone(),
            "sessionStartSource": "startup",
            "experimentalRawEvents": false,
        }),
    )
    .await
    {
        Ok(id) => id,
        Err(err) => {
            append_codex_app_server_log(format!("thread/start send failed: {err}"));
            let _ = app.emit(
                "agent-error",
                AgentErrorPayload {
                    conversation_id: conversation_id.clone(),
                    assistant_message_id: None,
                    message: err,
                },
            );
            let _ = child.kill().await;
            let _ = agent_session::update_agent_session_status(&db, &session_id, "idle").await;
            return;
        }
    };
    let thread_id = match read_app_server_response(&mut stdout_lines, thread_start_id)
        .await
        .and_then(|result| {
            extract_app_server_thread_id(&result).ok_or_else(|| {
                format!("Codex app-server thread/start response missing thread.id: {result}")
            })
        }) {
        Ok(thread_id) => thread_id,
        Err(err) => {
            append_codex_app_server_log(format!("thread/start failed: {err}"));
            let _ = app.emit(
                "agent-error",
                AgentErrorPayload {
                    conversation_id: conversation_id.clone(),
                    assistant_message_id: None,
                    message: err,
                },
            );
            let _ = child.kill().await;
            let _ = agent_session::update_agent_session_status(&db, &session_id, "idle").await;
            return;
        }
    };

    let turn_id = match send_app_server_request(
        &mut stdin,
        &mut next_request_id,
        "turn/start",
        serde_json::json!({
            "threadId": thread_id,
            "input": [{
                "type": "text",
                "text": prompt,
                "textElements": [],
            }],
            "collaborationMode": if plan_mode {
                serde_json::json!({
                    "mode": "plan",
                    "settings": {
                        "model": model_id,
                        "developerInstructions": null
                    }
                })
            } else {
                Value::Null
            },
            "cwd": cwd,
        }),
    )
    .await
    {
        Ok(id) => id,
        Err(err) => {
            append_codex_app_server_log(format!("turn/start send failed: {err}"));
            let _ = app.emit(
                "agent-error",
                AgentErrorPayload {
                    conversation_id: conversation_id.clone(),
                    assistant_message_id: None,
                    message: err,
                },
            );
            let _ = child.kill().await;
            let _ = agent_session::update_agent_session_status(&db, &session_id, "idle").await;
            return;
        }
    };
    append_codex_app_server_log(format!("request turn/start thread_id={thread_id}"));

    append_codex_app_server_log(format!("response thread/start ok thread_id={thread_id}"));

    let mut turn_started_ack = false;
    loop {
        tokio::select! {
            _ = async {
                while !cancel_token.is_cancelled() {
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                }
            } => {
                let _ = child.kill().await;
                let _ = app.emit(
                    "agent-error",
                    AgentErrorPayload {
                        conversation_id: conversation_id.clone(),
                        assistant_message_id: current_assistant_msg_id.clone(),
                        message: "Codex app-server run cancelled".to_string(),
                    },
                );
                let _ = agent_session::update_agent_session_status(&db, &session_id, "idle").await;
                return;
            }
            line = stdout_lines.next_line() => {
                let line = match line {
                    Ok(Some(line)) => line,
                    Ok(None) => break,
                    Err(err) => {
                        append_codex_app_server_log(format!("stdout read failed: {err}"));
                        let _ = app.emit(
                            "agent-error",
                            AgentErrorPayload {
                                conversation_id: conversation_id.clone(),
                                assistant_message_id: current_assistant_msg_id.clone(),
                                message: format!("Failed to read Codex app-server output: {err}"),
                            },
                        );
                        let _ = child.kill().await;
                        let _ = agent_session::update_agent_session_status(&db, &session_id, "idle").await;
                        return;
                    }
                };
                if line.trim().is_empty() {
                    continue;
                }
                let value: Value = match serde_json::from_str(&line) {
                    Ok(value) => value,
                    Err(err) => {
                        append_codex_app_server_log(format!(
                            "invalid stdout JSON-RPC line: {err}: {line}"
                        ));
                        tracing::warn!("[agent:codex_app_server] invalid JSON-RPC line: {}: {}", err, line);
                        continue;
                    }
                };

                if value.get("id").and_then(|v| v.as_i64()) == Some(turn_id) {
                    if let Some(error) = value.get("error") {
                        let message = error
                            .get("message")
                            .and_then(|v| v.as_str())
                            .unwrap_or("Codex app-server turn/start failed")
                            .to_string();
                        append_codex_app_server_log(format!("turn/start failed: {message}"));
                        let _ = app.emit(
                            "agent-error",
                            AgentErrorPayload {
                                conversation_id: conversation_id.clone(),
                                assistant_message_id: current_assistant_msg_id.clone(),
                                message,
                            },
                        );
                        let _ = child.kill().await;
                        let _ = agent_session::update_agent_session_status(&db, &session_id, "idle").await;
                        return;
                    }
                    continue;
                }

                if let (Some(request_id), Some(method)) = (
                    value.get("id").and_then(|v| v.as_i64()),
                    value.get("method").and_then(|v| v.as_str()),
                ) {
                    let result = match method {
                        "item/commandExecution/requestApproval"
                        | "item/fileChange/requestApproval"
                        | "execCommandApproval"
                        | "applyPatchApproval" => Some(
                            match request_codex_permission(
                                &app,
                                &permission_senders,
                                &db,
                                &conversation_id,
                                &user_msg_id,
                                &mut current_assistant_msg_id,
                                &assistant_id_for_task,
                                method,
                                request_id,
                                value.get("params").unwrap_or(&Value::Null),
                            )
                            .await
                            {
                                Ok(result) => result,
                                Err(err) => {
                                    append_codex_app_server_log(format!(
                                        "permission request failed: {err}"
                                    ));
                                    serde_json::json!({ "decision": "decline" })
                                }
                            },
                        ),
                        _ => None,
                    };

                    let response = if let Some(result) = result {
                        serde_json::json!({
                            "id": request_id,
                            "result": result,
                        })
                    } else {
                        serde_json::json!({
                            "id": request_id,
                            "error": {
                                "code": -32601,
                                "message": format!("FrogClaw does not implement Codex app-server request method: {method}"),
                            },
                        })
                    };
                    if let Err(err) = write_app_server_message(&mut stdin, &response).await {
                        let _ = app.emit(
                            "agent-error",
                            AgentErrorPayload {
                                conversation_id: conversation_id.clone(),
                                assistant_message_id: current_assistant_msg_id.clone(),
                                message: err,
                            },
                        );
                        let _ = child.kill().await;
                        let _ = agent_session::update_agent_session_status(&db, &session_id, "idle").await;
                        return;
                    }
                    continue;
                }

                let method = value.get("method").and_then(|v| v.as_str()).unwrap_or_default();
                let params = value.get("params").cloned().unwrap_or(Value::Null);
                if let Some(usage) = extract_codex_usage(&params).or_else(|| extract_codex_usage(&value)) {
                    final_usage = Some(usage);
                }
                if !method.is_empty() {
                    append_codex_app_server_event_log(method, &params);
                }
                match method {
                    "turn/started" => {
                        turn_started_ack = true;
                    }
                    "item/started" => {
                        if let Some((tool_use_id, tool_name, input)) =
                            extract_app_server_tool_info(&params)
                        {
                            tool_names.insert(tool_use_id.clone(), tool_name.clone());
                            let assistant_message_id = ensure_agent_assistant_message(
                                &db,
                                &app,
                                &conversation_id,
                                &user_msg_id,
                                &accumulated_text,
                                &mut current_assistant_msg_id,
                                &assistant_id_for_task,
                            )
                            .await
                            .unwrap_or_default();
                            append_tool_call_tag(&mut accumulated_text, &tool_use_id, &tool_name);
                            let _ = message::update_message_content(
                                &db,
                                &assistant_message_id,
                                &accumulated_text,
                            )
                            .await;
                            let _ = app.emit(
                                "agent-stream-text",
                                AgentTextPayload {
                                    conversation_id: conversation_id.clone(),
                                    assistant_message_id: assistant_message_id.clone(),
                                    text: String::new(),
                                },
                            );
                            let _ = app.emit(
                                "agent-tool-start",
                                AgentToolStartPayload {
                                    conversation_id: conversation_id.clone(),
                                    assistant_message_id,
                                    tool_use_id,
                                    tool_name,
                                    input,
                                },
                            );
                        }
                    }
                    "item/commandExecution/outputDelta" => {
                        if let Some((tool_use_id, delta)) = extract_app_server_output_delta(&params)
                        {
                            tool_outputs
                                .entry(tool_use_id)
                                .and_modify(|output| output.push_str(&delta))
                                .or_insert(delta);
                        }
                    }
                    "turn/plan/updated" => {
                        if let Some(next_plan) = extract_app_server_plan_update(&params) {
                            plan_text = next_plan;
                        }
                    }
                    "item/plan/delta" => {
                        if let Some(delta) = extract_app_server_plan_delta(&params) {
                            plan_text.push_str(&delta);
                        }
                    }
                    "item/agentMessage/delta" => {
                        if let Some(delta) = params.get("delta").and_then(|v| v.as_str()) {
                            if delta.is_empty() {
                                continue;
                            }
                            saw_agent_delta = true;
                            accumulated_text.push_str(delta);
                            let assistant_message_id = persist_agent_partial_content(
                                &db,
                                &app,
                                &conversation_id,
                                &user_msg_id,
                                &accumulated_text,
                                &mut current_assistant_msg_id,
                                &assistant_id_for_task,
                            )
                            .await
                            .unwrap_or_default();
                            let _ = app.emit(
                                "agent-stream-text",
                                AgentTextPayload {
                                    conversation_id: conversation_id.clone(),
                                    assistant_message_id,
                                    text: delta.to_string(),
                                },
                            );
                        }
                    }
                    "item/completed" => {
                        if let Some((tool_use_id, tool_name, _input)) =
                            extract_app_server_tool_info(&params)
                        {
                            let assistant_message_id =
                                current_assistant_msg_id.clone().unwrap_or_default();
                            let output = tool_outputs.remove(&tool_use_id).unwrap_or_else(|| {
                                extract_app_server_item(&params)
                                    .and_then(|item| {
                                        item.get("output")
                                            .or_else(|| item.get("result"))
                                            .and_then(|v| v.as_str())
                                    })
                                    .unwrap_or_default()
                                    .to_string()
                            });
                            let name = tool_names.remove(&tool_use_id).unwrap_or(tool_name);
                            let _ = app.emit(
                                "agent-tool-result",
                                AgentToolResultPayload {
                                    conversation_id: conversation_id.clone(),
                                    assistant_message_id,
                                    tool_use_id,
                                    tool_name: name,
                                    content: output,
                                    is_error: false,
                                },
                            );
                            continue;
                        }
                        if let Some(text) = extract_app_server_completed_plan_text(&params) {
                            plan_text = text;
                            continue;
                        }
                        if !saw_agent_delta {
                            if let Some(text) = extract_app_server_completed_text(&params) {
                                accumulated_text.push_str(&text);
                                let assistant_message_id = persist_agent_partial_content(
                                    &db,
                                    &app,
                                    &conversation_id,
                                    &user_msg_id,
                                    &accumulated_text,
                                    &mut current_assistant_msg_id,
                                    &assistant_id_for_task,
                                )
                                .await
                                .unwrap_or_default();
                                let _ = app.emit(
                                    "agent-stream-text",
                                    AgentTextPayload {
                                        conversation_id: conversation_id.clone(),
                                        assistant_message_id,
                                        text,
                                    },
                                );
                            }
                        }
                    }
                    "error" => {
                        let will_retry = params.get("willRetry").and_then(|v| v.as_bool()).unwrap_or(false);
                        if !will_retry {
                            let message = params
                                .get("error")
                                .and_then(|err| err.get("message").or_else(|| err.get("details")))
                                .and_then(|v| v.as_str())
                                .unwrap_or("Codex app-server turn failed")
                                .to_string();
                            let _ = app.emit(
                                "agent-error",
                                AgentErrorPayload {
                                    conversation_id: conversation_id.clone(),
                                    assistant_message_id: current_assistant_msg_id.clone(),
                                    message,
                                },
                            );
                            let _ = child.kill().await;
                            let _ = agent_session::update_agent_session_status(&db, &session_id, "idle").await;
                            return;
                        }
                    }
                    "turn/completed" => {
                        if plan_mode && !plan_text.trim().is_empty() {
                            let text = plan_text.trim().to_string();
                            if accumulated_text.trim().is_empty()
                                || accumulated_text.contains("<tool-call")
                            {
                                if !accumulated_text.trim().is_empty() {
                                    accumulated_text.push_str("\n\n");
                                }
                                accumulated_text.push_str(&text);
                                let assistant_message_id = persist_agent_partial_content(
                                    &db,
                                    &app,
                                    &conversation_id,
                                    &user_msg_id,
                                    &accumulated_text,
                                    &mut current_assistant_msg_id,
                                    &assistant_id_for_task,
                                )
                                .await
                                .unwrap_or_default();
                                let _ = app.emit(
                                    "agent-stream-text",
                                    AgentTextPayload {
                                        conversation_id: conversation_id.clone(),
                                        assistant_message_id,
                                        text,
                                    },
                                );
                            }
                        }
                        let assistant_message_id = if let Some(id) = current_assistant_msg_id.clone() {
                            id
                        } else {
                            let fallback = if accumulated_text.is_empty() {
                                "Codex app-server completed without text output.".to_string()
                            } else {
                                accumulated_text.clone()
                            };
                            match message::create_message(
                                &db,
                                &conversation_id,
                                MessageRole::Assistant,
                                &fallback,
                                &[],
                                Some(&user_msg_id),
                                0,
                            )
                            .await
                            {
                                Ok(msg) => {
                                    let _ = conversation::increment_message_count(&db, &conversation_id).await;
                                    msg.id
                                }
                                Err(_) => String::new(),
                            }
                        };
                        let effective_usage =
                            effective_agent_usage(final_usage.as_ref(), &prompt, &accumulated_text);
                        let _ = app.emit(
                            "agent-done",
                            AgentDonePayload {
                                conversation_id: conversation_id.clone(),
                                assistant_message_id: assistant_message_id.clone(),
                                text: accumulated_text.clone(),
                                usage: Some(AgentUsagePayload {
                                    input_tokens: effective_usage.input_tokens,
                                    output_tokens: effective_usage.output_tokens,
                                }),
                                num_turns: None,
                                cost_usd: None,
                            },
                        );
                        let _ = message::update_message_usage(
                            &db,
                            &assistant_message_id,
                            Some(effective_usage.input_tokens as i64),
                            Some(effective_usage.output_tokens as i64),
                        )
                        .await;
                        if !plan_mode && !accumulated_text.trim().is_empty() {
                            if let Ok(mut conversation) =
                                conversation::get_conversation(&db, &conversation_id).await
                            {
                                if let Some(ref cwd) = cwd {
                                    if !cwd.trim().is_empty() {
                                        conversation.working_directory = Some(cwd.clone());
                                        if conversation.project_name.as_deref().is_none_or(|v| v.trim().is_empty()) {
                                            conversation.project_name = std::path::Path::new(cwd)
                                                .file_name()
                                                .and_then(|name| name.to_str())
                                                .map(ToString::to_string);
                                        }
                                    }
                                }
                                crate::commands::conversations::auto_capture_project_memory(
                                    &db,
                                    &master_key,
                                    vector_store.as_ref(),
                                    &provider,
                                    &provider_ctx,
                                    &conversation,
                                    &model_id,
                                    &original_prompt,
                                    &accumulated_text,
                                )
                                .await;
                            }
                        }
                        let _ = child.kill().await;
                        let _ = agent_session::update_agent_session_status(&db, &session_id, "idle").await;
                        if is_first_message {
                            tracing::info!("[agent:codex_app_server] first message completed; title fallback already set");
                        }
                        return;
                    }
                    _ => {}
                }
            }
            line = async {
                match stderr_lines.as_mut() {
                    Some(lines) => lines.next_line().await,
                    None => Ok(None),
                }
            } => {
                if let Ok(Some(line)) = line {
                    if !line.trim().is_empty() {
                        append_codex_app_server_log(format!("stderr: {line}"));
                    }
                }
            }
        }
    }

    let status = child.wait().await;
    let message = match status {
        Ok(status) if status.success() && !turn_started_ack => {
            "Codex app-server exited before the turn started".to_string()
        }
        Ok(status) => format!("Codex app-server exited before turn completion: {status}"),
        Err(err) => format!("Failed to wait for Codex app-server: {err}"),
    };
    append_codex_app_server_log(&message);
    let _ = app.emit(
        "agent-error",
        AgentErrorPayload {
            conversation_id: conversation_id.clone(),
            assistant_message_id: current_assistant_msg_id,
            message,
        },
    );
    let _ = agent_session::update_agent_session_status(&db, &session_id, "idle").await;
}

async fn run_claude_code_cli_query(
    app: tauri::AppHandle,
    db: sea_orm::DatabaseConnection,
    state_tokens: Arc<tokio::sync::Mutex<HashMap<String, AgentCancellationToken>>>,
    conversation_id: String,
    session_id: String,
    user_msg_id: String,
    prompt: String,
    cwd: Option<String>,
    permission_mode: String,
    is_first_message: bool,
) {
    let run_id = format!("claude_{}", frogclaw_core::utils::gen_id());
    if let Ok(mut running) = RUNNING_AGENTS.lock() {
        running.insert(conversation_id.clone(), run_id.clone());
    }
    let _guard = RunningAgentGuard {
        conversation_id: conversation_id.clone(),
        run_id,
    };

    let cancel_token = AgentCancellationToken::new();
    state_tokens
        .lock()
        .await
        .insert(conversation_id.clone(), cancel_token.clone());
    let _cancel_guard = AgentCancelTokenGuard {
        conversation_id: conversation_id.clone(),
        tokens: state_tokens.clone(),
    };

    let claude_path = match find_binary("claude", &claude_candidate_dirs()) {
        Some(path) => path,
        None => {
            let _ = app.emit(
                "agent-error",
                AgentErrorPayload {
                    conversation_id: conversation_id.clone(),
                    assistant_message_id: None,
                    message: "Claude Code CLI not found. Install and login to Claude Code first."
                        .to_string(),
                },
            );
            let _ = agent_session::update_agent_session_status(&db, &session_id, "idle").await;
            return;
        }
    };

    let mut current_assistant_msg_id: Option<String> = None;
    let assistant_id_for_task: Arc<RwLock<Option<String>>> = Arc::new(RwLock::new(None));
    let mut accumulated_text = String::new();

    let mut cmd = tokio::process::Command::new(&claude_path);
    cmd.arg("-p")
        .arg("--output-format")
        .arg("stream-json")
        .arg("--verbose")
        .arg(&prompt)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());

    if permission_mode == "accept_edits" {
        cmd.arg("--permission-mode").arg("acceptEdits");
    }
    if let Some(cwd) = cwd.as_deref() {
        cmd.current_dir(cwd);
    }

    #[cfg(windows)]
    {
        cmd.creation_flags(0x08000000);
    }

    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(err) => {
            let _ = app.emit(
                "agent-error",
                AgentErrorPayload {
                    conversation_id: conversation_id.clone(),
                    assistant_message_id: None,
                    message: format!("Failed to start Claude Code CLI: {err}"),
                },
            );
            let _ = agent_session::update_agent_session_status(&db, &session_id, "idle").await;
            return;
        }
    };

    let stdout = child.stdout.take();
    let stderr = child.stderr.take();

    let _ = app.emit(
        "agent-status",
        AgentStatusPayload {
            conversation_id: conversation_id.clone(),
            message: "Claude Code running".to_string(),
        },
    );

    let mut stdout_lines = stdout.map(|out| BufReader::new(out).lines());
    let mut stderr_lines = stderr.map(|err| BufReader::new(err).lines());

    loop {
        tokio::select! {
            _ = async {
                while !cancel_token.is_cancelled() {
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                }
            } => {
                let _ = child.kill().await;
                let _ = app.emit(
                    "agent-error",
                    AgentErrorPayload {
                        conversation_id: conversation_id.clone(),
                        assistant_message_id: current_assistant_msg_id.clone(),
                        message: "Claude Code run cancelled".to_string(),
                    },
                );
                let _ = agent_session::update_agent_session_status(&db, &session_id, "idle").await;
                return;
            }
            line = async {
                match stdout_lines.as_mut() {
                    Some(lines) => lines.next_line().await,
                    None => Ok(None),
                }
            } => {
                match line {
                    Ok(Some(line)) => {
                        if line.trim().is_empty() {
                            continue;
                        }
                        if let Ok(value) = serde_json::from_str::<Value>(&line) {
                            let event_type = value.get("type").and_then(|v| v.as_str()).unwrap_or_default();
                            if event_type == "system" {
                                if let Some(subtype) = value.get("subtype").and_then(|v| v.as_str()) {
                                    let _ = app.emit(
                                        "agent-status",
                                        AgentStatusPayload {
                                            conversation_id: conversation_id.clone(),
                                            message: format!("Claude Code: {subtype}"),
                                        },
                                    );
                                }
                            }
                            let mut texts = extract_claude_texts(&value);
                            if texts.is_empty() && event_type == "result" && accumulated_text.is_empty() {
                                if let Some(result) = value.get("result").and_then(|v| v.as_str()) {
                                    if !result.trim().is_empty() {
                                        texts.push(result.to_string());
                                    }
                                }
                            }
                            for text in texts {
                                if !accumulated_text.is_empty() {
                                    accumulated_text.push_str("\n\n");
                                }
                                accumulated_text.push_str(&text);
                                let assistant_message_id = persist_agent_partial_content(
                                    &db,
                                    &app,
                                    &conversation_id,
                                    &user_msg_id,
                                    &accumulated_text,
                                    &mut current_assistant_msg_id,
                                    &assistant_id_for_task,
                                )
                                .await
                                .unwrap_or_default();
                                let _ = app.emit(
                                    "agent-stream-text",
                                    AgentTextPayload {
                                        conversation_id: conversation_id.clone(),
                                        assistant_message_id,
                                        text,
                                    },
                                );
                            }
                        } else {
                            let assistant_message_id = persist_agent_partial_content(
                                &db,
                                &app,
                                &conversation_id,
                                &user_msg_id,
                                &line,
                                &mut current_assistant_msg_id,
                                &assistant_id_for_task,
                            )
                            .await
                            .unwrap_or_default();
                            let _ = app.emit(
                                "agent-stream-text",
                                AgentTextPayload {
                                    conversation_id: conversation_id.clone(),
                                    assistant_message_id,
                                    text: line,
                                },
                            );
                        }
                    }
                    Ok(None) => break,
                    Err(err) => {
                        tracing::warn!("[agent:claude_code] stdout read error: {}", err);
                        break;
                    }
                }
            }
            line = async {
                match stderr_lines.as_mut() {
                    Some(lines) => lines.next_line().await,
                    None => Ok(None),
                }
            } => {
                if let Ok(Some(line)) = line {
                    if !line.trim().is_empty() {
                        let _ = app.emit(
                            "agent-status",
                            AgentStatusPayload {
                                conversation_id: conversation_id.clone(),
                                message: line.chars().take(160).collect(),
                            },
                        );
                    }
                }
            }
        }
    }

    match child.wait().await {
        Ok(status) if status.success() => {
            let assistant_message_id = if let Some(id) = current_assistant_msg_id.clone() {
                id
            } else {
                let fallback = if accumulated_text.is_empty() {
                    "Claude Code completed without text output.".to_string()
                } else {
                    accumulated_text.clone()
                };
                match message::create_message(
                    &db,
                    &conversation_id,
                    MessageRole::Assistant,
                    &fallback,
                    &[],
                    Some(&user_msg_id),
                    0,
                )
                .await
                {
                    Ok(msg) => {
                        let _ = conversation::increment_message_count(&db, &conversation_id).await;
                        msg.id
                    }
                    Err(_) => String::new(),
                }
            };

            let _ = app.emit(
                "agent-done",
                AgentDonePayload {
                    conversation_id: conversation_id.clone(),
                    assistant_message_id: assistant_message_id.clone(),
                    text: accumulated_text,
                    usage: None,
                    num_turns: None,
                    cost_usd: None,
                },
            );
            let _ = agent_session::update_agent_session_status(&db, &session_id, "idle").await;
            if is_first_message {
                tracing::info!(
                    "[agent:claude_code] first message completed; title fallback already set"
                );
            }
        }
        Ok(status) => {
            let _ = app.emit(
                "agent-error",
                AgentErrorPayload {
                    conversation_id: conversation_id.clone(),
                    assistant_message_id: current_assistant_msg_id,
                    message: format!("Claude Code exited with status: {status}"),
                },
            );
            let _ = agent_session::update_agent_session_status(&db, &session_id, "idle").await;
        }
        Err(err) => {
            let _ = app.emit(
                "agent-error",
                AgentErrorPayload {
                    conversation_id: conversation_id.clone(),
                    assistant_message_id: current_assistant_msg_id,
                    message: format!("Failed to wait for Claude Code: {err}"),
                },
            );
            let _ = agent_session::update_agent_session_status(&db, &session_id, "idle").await;
        }
    }
}

async fn run_codex_cli_query(
    app: tauri::AppHandle,
    db: sea_orm::DatabaseConnection,
    state_tokens: Arc<tokio::sync::Mutex<HashMap<String, AgentCancellationToken>>>,
    conversation_id: String,
    session_id: String,
    user_msg_id: String,
    prompt: String,
    cwd: Option<String>,
    permission_mode: String,
    extra_env: Option<HashMap<String, String>>,
    is_first_message: bool,
) {
    let run_id = format!("codex_{}", frogclaw_core::utils::gen_id());
    if let Ok(mut running) = RUNNING_AGENTS.lock() {
        running.insert(conversation_id.clone(), run_id.clone());
    }
    let _guard = RunningAgentGuard {
        conversation_id: conversation_id.clone(),
        run_id,
    };

    let cancel_token = AgentCancellationToken::new();
    state_tokens
        .lock()
        .await
        .insert(conversation_id.clone(), cancel_token.clone());
    let _cancel_guard = AgentCancelTokenGuard {
        conversation_id: conversation_id.clone(),
        tokens: state_tokens.clone(),
    };

    let codex_path = match find_binary("codex", &claude_candidate_dirs()) {
        Some(path) => path,
        None => {
            let _ = app.emit(
                "agent-error",
                AgentErrorPayload {
                    conversation_id: conversation_id.clone(),
                    assistant_message_id: None,
                    message: "Codex CLI not found. Install and login to Codex first.".to_string(),
                },
            );
            let _ = agent_session::update_agent_session_status(&db, &session_id, "idle").await;
            return;
        }
    };

    let mut current_assistant_msg_id: Option<String> = None;
    let assistant_id_for_task: Arc<RwLock<Option<String>>> = Arc::new(RwLock::new(None));
    let mut accumulated_text = String::new();
    let mut final_usage: Option<CodexUsage> = None;

    let mut cmd = tokio::process::Command::new(&codex_path);
    cmd.arg("exec")
        .arg("--json")
        .arg("--skip-git-repo-check")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .stdin(std::process::Stdio::null());

    match permission_mode.as_str() {
        "full_access" => {
            cmd.arg("--dangerously-bypass-approvals-and-sandbox");
        }
        "accept_edits" => {
            cmd.arg("--sandbox").arg("workspace-write");
        }
        _ => {
            cmd.arg("--sandbox").arg("workspace-write");
        }
    }

    if let Some(cwd) = cwd.as_deref() {
        cmd.arg("-C").arg(cwd);
        cmd.current_dir(cwd);
    }
    if let Some(env) = extra_env {
        cmd.envs(env);
    }
    cmd.arg(&prompt);

    #[cfg(windows)]
    {
        cmd.creation_flags(0x08000000);
    }

    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(err) => {
            let _ = app.emit(
                "agent-error",
                AgentErrorPayload {
                    conversation_id: conversation_id.clone(),
                    assistant_message_id: None,
                    message: format!("Failed to start Codex CLI: {err}"),
                },
            );
            let _ = agent_session::update_agent_session_status(&db, &session_id, "idle").await;
            return;
        }
    };

    let stdout = child.stdout.take();
    let stderr = child.stderr.take();

    let _ = app.emit(
        "agent-status",
        AgentStatusPayload {
            conversation_id: conversation_id.clone(),
            message: "Codex CLI running".to_string(),
        },
    );

    let mut stdout_lines = stdout.map(|out| BufReader::new(out).lines());
    let mut stderr_lines = stderr.map(|err| BufReader::new(err).lines());

    loop {
        tokio::select! {
            _ = async {
                while !cancel_token.is_cancelled() {
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                }
            } => {
                let _ = child.kill().await;
                let _ = app.emit(
                    "agent-error",
                    AgentErrorPayload {
                        conversation_id: conversation_id.clone(),
                        assistant_message_id: current_assistant_msg_id.clone(),
                        message: "Codex CLI run cancelled".to_string(),
                    },
                );
                let _ = agent_session::update_agent_session_status(&db, &session_id, "idle").await;
                return;
            }
            line = async {
                match stdout_lines.as_mut() {
                    Some(lines) => lines.next_line().await,
                    None => Ok(None),
                }
            } => {
                match line {
                    Ok(Some(line)) => {
                        if line.trim().is_empty() {
                            continue;
                        }
                        if let Ok(value) = serde_json::from_str::<Value>(&line) {
                            if let Some(usage) = extract_codex_usage(&value) {
                                final_usage = Some(usage);
                            }
                            if let Some(event_type) = value.get("type").and_then(|v| v.as_str()) {
                                match event_type {
                                    "thread.started" => {
                                        if let Some(thread_id) = value.get("thread_id").and_then(|v| v.as_str()) {
                                            let _ = app.emit(
                                                "agent-status",
                                                AgentStatusPayload {
                                                    conversation_id: conversation_id.clone(),
                                                    message: format!("Codex CLI thread {thread_id}"),
                                                },
                                            );
                                        }
                                    }
                                    "turn.started" => {
                                        let _ = app.emit(
                                            "agent-status",
                                            AgentStatusPayload {
                                                conversation_id: conversation_id.clone(),
                                                message: "Codex CLI thinking".to_string(),
                                            },
                                        );
                                    }
                                    _ => {}
                                }
                            }
                            for text in extract_codex_texts(&value) {
                                if !accumulated_text.is_empty() {
                                    accumulated_text.push_str("\n\n");
                                }
                                accumulated_text.push_str(&text);
                                let assistant_message_id = persist_agent_partial_content(
                                    &db,
                                    &app,
                                    &conversation_id,
                                    &user_msg_id,
                                    &accumulated_text,
                                    &mut current_assistant_msg_id,
                                    &assistant_id_for_task,
                                )
                                .await
                                .unwrap_or_default();
                                let _ = app.emit(
                                    "agent-stream-text",
                                    AgentTextPayload {
                                        conversation_id: conversation_id.clone(),
                                        assistant_message_id,
                                        text,
                                    },
                                );
                            }
                        }
                    }
                    Ok(None) => break,
                    Err(err) => {
                        tracing::warn!("[agent:codex_cli] stdout read error: {}", err);
                        break;
                    }
                }
            }
            line = async {
                match stderr_lines.as_mut() {
                    Some(lines) => lines.next_line().await,
                    None => Ok(None),
                }
            } => {
                if let Ok(Some(line)) = line {
                    if !line.trim().is_empty() {
                        let _ = app.emit(
                            "agent-status",
                            AgentStatusPayload {
                                conversation_id: conversation_id.clone(),
                                message: line.chars().take(160).collect(),
                            },
                        );
                    }
                }
            }
        }
    }

    match child.wait().await {
        Ok(status) if status.success() => {
            let assistant_message_id = if let Some(id) = current_assistant_msg_id.clone() {
                id
            } else {
                let fallback = if accumulated_text.is_empty() {
                    "Codex CLI completed without text output.".to_string()
                } else {
                    accumulated_text.clone()
                };
                match message::create_message(
                    &db,
                    &conversation_id,
                    MessageRole::Assistant,
                    &fallback,
                    &[],
                    Some(&user_msg_id),
                    0,
                )
                .await
                {
                    Ok(msg) => {
                        let _ = conversation::increment_message_count(&db, &conversation_id).await;
                        msg.id
                    }
                    Err(_) => String::new(),
                }
            };

            let effective_usage =
                effective_agent_usage(final_usage.as_ref(), &prompt, &accumulated_text);
            let _ = app.emit(
                "agent-done",
                AgentDonePayload {
                    conversation_id: conversation_id.clone(),
                    assistant_message_id: assistant_message_id.clone(),
                    text: accumulated_text,
                    usage: Some(AgentUsagePayload {
                        input_tokens: effective_usage.input_tokens,
                        output_tokens: effective_usage.output_tokens,
                    }),
                    num_turns: None,
                    cost_usd: None,
                },
            );
            let _ = message::update_message_usage(
                &db,
                &assistant_message_id,
                Some(effective_usage.input_tokens as i64),
                Some(effective_usage.output_tokens as i64),
            )
            .await;
            let _ = agent_session::update_agent_session_status(&db, &session_id, "idle").await;
            if is_first_message {
                tracing::info!(
                    "[agent:codex_cli] first message completed; title fallback already set"
                );
            }
        }
        Ok(status) => {
            let _ = app.emit(
                "agent-error",
                AgentErrorPayload {
                    conversation_id: conversation_id.clone(),
                    assistant_message_id: current_assistant_msg_id,
                    message: format!("Codex CLI exited with status: {status}"),
                },
            );
            let _ = agent_session::update_agent_session_status(&db, &session_id, "idle").await;
        }
        Err(err) => {
            let _ = app.emit(
                "agent-error",
                AgentErrorPayload {
                    conversation_id: conversation_id.clone(),
                    assistant_message_id: current_assistant_msg_id,
                    message: format!("Failed to wait for Codex CLI: {err}"),
                },
            );
            let _ = agent_session::update_agent_session_status(&db, &session_id, "idle").await;
        }
    }
}

#[tauri::command]
pub async fn agent_get_session(
    state: State<'_, AppState>,
    conversation_id: String,
) -> Result<Option<AgentSession>, String> {
    agent_session::get_agent_session_by_conversation_id(&state.sea_db, &conversation_id)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn agent_list_engines() -> Result<Vec<AgentEngineInfo>, String> {
    let claude_dirs = claude_candidate_dirs();
    let codex_dirs = claude_candidate_dirs();
    let gemini_dirs = Vec::new();

    Ok(vec![
        AgentEngineInfo {
            kind: ENGINE_CODEX_APP_SERVER.to_string(),
            display_name: "Codex App Server".to_string(),
            description: "Codex runtime using FrogClaw-generated config under ~/.frogclaw/codex."
                .to_string(),
            available: true,
            installed: true,
            version: None,
            binary_path: None,
            status: "available".to_string(),
            message: None,
            experimental: false,
        },
        AgentEngineInfo {
            kind: ENGINE_FROG_AGENT.to_string(),
            display_name: "Frog Agent (legacy alias)".to_string(),
            description: "Legacy engine value routed to Codex App Server for compatibility."
                .to_string(),
            available: true,
            installed: true,
            version: None,
            binary_path: None,
            status: "available".to_string(),
            message: Some("Uses Codex App Server runtime".to_string()),
            experimental: false,
        },
        cli_engine_info(
            ENGINE_CLAUDE_CODE,
            "Claude Code",
            "Claude Code CLI engine with local coding-agent capabilities.",
            "claude",
            &claude_dirs,
            false,
        ),
        cli_engine_info(
            ENGINE_CODEX_CLI,
            "Codex CLI",
            "Codex CLI engine with local coding-agent capabilities.",
            "codex",
            &codex_dirs,
            false,
        ),
        cli_engine_info(
            ENGINE_GEMINI_CLI,
            "Gemini CLI",
            "Experimental Gemini CLI engine placeholder.",
            "gemini",
            &gemini_dirs,
            true,
        ),
    ])
}

/// Create default workspace directory under config home and return its path.
#[tauri::command]
pub async fn agent_ensure_workspace(conversation_id: String) -> Result<String, String> {
    let workspace_dir = crate::paths::frogclaw_home()
        .join("workspace")
        .join(&conversation_id);
    std::fs::create_dir_all(&workspace_dir)
        .map_err(|e| format!("Failed to create workspace: {}", e))?;
    workspace_dir
        .to_str()
        .map(|s| s.to_string())
        .ok_or_else(|| "Invalid path encoding".to_string())
}

/// Backup and clear SDK context when a context-clear marker is inserted.
#[tauri::command]
pub async fn agent_backup_and_clear_sdk_context(
    state: State<'_, AppState>,
    conversation_id: String,
) -> Result<(), String> {
    agent_session::backup_and_clear_sdk_context_by_conversation_id(&state.sea_db, &conversation_id)
        .await
        .map_err(|e| e.to_string())
}

/// Restore SDK context from backup when a context-clear marker is removed.
#[tauri::command]
pub async fn agent_restore_sdk_context_from_backup(
    state: State<'_, AppState>,
    conversation_id: String,
) -> Result<(), String> {
    agent_session::restore_sdk_context_from_backup_by_conversation_id(
        &state.sea_db,
        &conversation_id,
    )
    .await
    .map_err(|e| e.to_string())
}
