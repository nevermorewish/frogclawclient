use crate::AppState;
use axum::{
    extract::State as AxumState,
    response::sse::{Event, KeepAlive, Sse},
    routing::post,
    Json, Router,
};
use futures::{Stream, StreamExt};
use frogclaw_core::types::*;
use frogclaw_providers::{registry::ProviderRegistry, resolve_base_url_for_type, ProviderAdapter, ProviderRequestContext};
use sea_orm::DatabaseConnection;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    io::{Read, Write},
    net::{SocketAddr, TcpStream},
    path::{Path, PathBuf},
    pin::Pin,
    process::{Child, Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;
use tauri::State;
use tokio::sync::{mpsc, Mutex};
use tokio_stream::wrappers::ReceiverStream;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImChannel {
    pub id: String,
    pub platform: String,
    #[serde(rename = "appId", alias = "app_id")]
    pub app_id: String,
    #[serde(rename = "appSecret", alias = "app_secret")]
    pub app_secret: String,
    pub label: Option<String>,
    #[serde(default = "default_channel_enabled")]
    pub enabled: bool,
    pub assignment: Option<String>,
    pub sandbox: Option<bool>,
}

fn default_channel_enabled() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct ImChannelsFile {
    channels: Vec<ImChannel>,
}

#[derive(Debug, Serialize)]
pub struct PlatformStatus {
    pub running: bool,
    pub parent_port: Option<u16>,
    pub config_path: String,
    pub log_path: String,
}

#[derive(Debug, Deserialize)]
struct BridgeMessageRequest {
    #[serde(rename = "sessionKey")]
    session_key: String,
    prompt: String,
    files: Option<Vec<String>>,
    assignment: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SessionRequest {
    #[serde(rename = "sessionKey")]
    session_key: String,
}

#[derive(Clone)]
struct BridgeRuntime {
    db: DatabaseConnection,
    master_key: [u8; 32],
    sessions: Arc<Mutex<HashMap<String, String>>>,
    cancel_flags: Arc<Mutex<HashMap<String, Arc<AtomicBool>>>>,
}

pub struct PlatformBridgeState {
    child: Mutex<Option<Child>>,
    parent_port: Mutex<Option<u16>>,
    sessions: Arc<Mutex<HashMap<String, String>>>,
    cancel_flags: Arc<Mutex<HashMap<String, Arc<AtomicBool>>>>,
}

impl Default for PlatformBridgeState {
    fn default() -> Self {
        Self {
            child: Mutex::new(None),
            parent_port: Mutex::new(None),
            sessions: Arc::new(Mutex::new(HashMap::new())),
            cancel_flags: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

fn provider_type_to_registry_key(pt: &ProviderType) -> &'static str {
    match pt {
        ProviderType::OpenAI => "openai",
        ProviderType::OpenAIResponses => "openai_responses",
        ProviderType::Anthropic => "anthropic",
        ProviderType::Gemini => "gemini",
        ProviderType::Jina => "jina",
        ProviderType::Cohere => "cohere",
        ProviderType::Voyage => "voyage",
        ProviderType::Custom => "openai",
    }
}

fn config_dir() -> Result<PathBuf, String> {
    let home = dirs::home_dir().ok_or_else(|| "Unable to resolve home directory".to_string())?;
    let dir = home.join(".frogclaw");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir)
}

fn channels_path() -> Result<PathBuf, String> {
    Ok(config_dir()?.join("im-channels.json"))
}

fn log_path() -> Result<PathBuf, String> {
    Ok(config_dir()?.join("platform-sidecar.log"))
}

fn normalize_im_assignment(value: Option<String>) -> Option<String> {
    match value.as_deref() {
        Some("native_cli") | Some("none") => Some("native_cli".to_string()),
        _ => Some("aiagent".to_string()),
    }
}

fn is_platform_port_open() -> bool {
    let addr = SocketAddr::from(([127, 0, 0, 1], 18788));
    TcpStream::connect_timeout(&addr, Duration::from_millis(150)).is_ok()
}

fn request_platform_reload() -> bool {
    let addr = SocketAddr::from(([127, 0, 0, 1], 18788));
    let mut stream = match TcpStream::connect_timeout(&addr, Duration::from_millis(300)) {
        Ok(stream) => stream,
        Err(_) => return false,
    };
    let _ = stream.set_read_timeout(Some(Duration::from_millis(500)));
    let _ = stream.set_write_timeout(Some(Duration::from_millis(500)));
    let request = "POST /reload HTTP/1.1\r\nHost: 127.0.0.1:18788\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
    if stream.write_all(request.as_bytes()).is_err() {
        return false;
    }
    let mut response = String::new();
    let _ = stream.read_to_string(&mut response);
    response.starts_with("HTTP/1.1 200") || response.starts_with("HTTP/1.0 200")
}

fn sidecar_path() -> Result<PathBuf, String> {
    let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
    let exe_parent = std::env::current_exe()
        .map_err(|e| e.to_string())?
        .parent()
        .unwrap_or(Path::new("."))
        .to_path_buf();
    let candidates = [
        cwd.join("src-tauri").join("binaries").join("frogclaw-platform-sidecar.cjs"),
        cwd.join("binaries").join("frogclaw-platform-sidecar.cjs"),
        exe_parent.join("frogclaw-platform-sidecar.cjs"),
        exe_parent.join("resources").join("frogclaw-platform-sidecar.cjs"),
        exe_parent.join("resources").join("binaries").join("frogclaw-platform-sidecar.cjs"),
    ];
    candidates
        .into_iter()
        .find(|p| p.exists())
        .ok_or_else(|| "Sidecar bundle not found. Run `pnpm build:sidecar` first.".to_string())
}

fn read_channels_file() -> Result<ImChannelsFile, String> {
    let path = channels_path()?;
    if !path.exists() {
        return Ok(ImChannelsFile::default());
    }
    let raw = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    let mut file: ImChannelsFile = serde_json::from_str(&raw).map_err(|e| e.to_string())?;
    for channel in &mut file.channels {
        channel.assignment = normalize_im_assignment(channel.assignment.take());
    }
    Ok(file)
}

fn write_channels_file(file: &ImChannelsFile) -> Result<(), String> {
    let path = channels_path()?;
    let tmp = path.with_extension("json.tmp");
    let mut normalized = file.clone();
    for channel in &mut normalized.channels {
        channel.assignment = normalize_im_assignment(channel.assignment.take());
    }
    let raw = serde_json::to_string_pretty(&normalized).map_err(|e| e.to_string())?;
    std::fs::write(&tmp, raw).map_err(|e| e.to_string())?;
    std::fs::rename(tmp, path).map_err(|e| e.to_string())
}

async fn start_parent_server(app_state: &AppState, bridge: &PlatformBridgeState) -> Result<u16, String> {
    if let Some(port) = *bridge.parent_port.lock().await {
        return Ok(port);
    }

    let runtime = BridgeRuntime {
        db: app_state.sea_db.clone(),
        master_key: app_state.master_key,
        sessions: bridge.sessions.clone(),
        cancel_flags: bridge.cancel_flags.clone(),
    };
    let app = Router::new()
        .route("/message", post(bridge_message))
        .route("/reset", post(bridge_reset))
        .route("/cancel", post(bridge_cancel))
        .with_state(runtime);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|e| e.to_string())?;
    let port = listener.local_addr().map_err(|e| e.to_string())?.port();
    tokio::spawn(async move {
        if let Err(e) = axum::serve(listener, app).await {
            tracing::error!("IM bridge parent server stopped: {}", e);
        }
    });
    *bridge.parent_port.lock().await = Some(port);
    Ok(port)
}

async fn choose_default_model(db: &DatabaseConnection) -> Result<(ProviderConfig, Model), String> {
    let settings = frogclaw_core::repo::settings::get_settings(db).await.unwrap_or_default();
    let providers = frogclaw_core::repo::provider::list_providers(db)
        .await
        .map_err(|e| e.to_string())?;

    if let (Some(provider_id), Some(model_id)) = (
        settings.default_provider_id.as_deref(),
        settings.default_model_id.as_deref(),
    ) {
        if let Some(provider) = providers.iter().find(|p| p.id == provider_id && p.enabled) {
            if let Some(model) = provider
                .models
                .iter()
                .find(|m| m.model_id == model_id && m.enabled)
                .cloned()
            {
                return Ok((provider.clone(), model));
            }
        }
    }

    for provider in providers.into_iter().filter(|p| p.enabled) {
        if let Some(model) = provider.models.iter().find(|m| m.enabled).cloned() {
            return Ok((provider, model));
        }
    }

    Err("No enabled chat model is configured".to_string())
}

async fn get_or_create_session_conversation(
    runtime: &BridgeRuntime,
    session_key: &str,
    first_prompt: &str,
) -> Result<(Conversation, ProviderConfig, Model), String> {
    if let Some(conversation_id) = runtime.sessions.lock().await.get(session_key).cloned() {
        let conversation = frogclaw_core::repo::conversation::get_conversation(&runtime.db, &conversation_id)
            .await
            .map_err(|e| e.to_string())?;
        let provider = frogclaw_core::repo::provider::get_provider(&runtime.db, &conversation.provider_id)
            .await
            .map_err(|e| e.to_string())?;
        let model = frogclaw_core::repo::provider::get_model(
            &runtime.db,
            &conversation.provider_id,
            &conversation.model_id,
        )
        .await
        .map_err(|e| e.to_string())?;
        return Ok((conversation, provider, model));
    }

    let (provider, model) = choose_default_model(&runtime.db).await?;
    let title_seed = if first_prompt.chars().count() > 24 {
        format!("{}...", first_prompt.chars().take(24).collect::<String>())
    } else {
        first_prompt.to_string()
    };
    let title = if title_seed.trim().is_empty() {
        "飞书对话".to_string()
    } else {
        format!("飞书 - {}", title_seed)
    };
    let conversation = frogclaw_core::repo::conversation::create_conversation(
        &runtime.db,
        &title,
        &model.model_id,
        &provider.id,
        None,
    )
    .await
    .map_err(|e| e.to_string())?;
    let default_workspace = crate::paths::default_workspace();
    let _ = std::fs::create_dir_all(&default_workspace);
    let project_name = default_workspace
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("workspace")
        .to_string();
    let conversation = frogclaw_core::repo::conversation::update_conversation(
        &runtime.db,
        &conversation.id,
        UpdateConversationInput {
            working_directory: Some(Some(default_workspace.to_string_lossy().to_string())),
            project_name: Some(Some(project_name)),
            ..Default::default()
        },
    )
    .await
    .map_err(|e| e.to_string())?;
    runtime
        .sessions
        .lock()
        .await
        .insert(session_key.to_string(), conversation.id.clone());
    Ok((conversation, provider, model))
}

fn message_to_chat(message: &Message) -> Option<ChatMessage> {
    if message.status == "error" || message.role == MessageRole::Tool {
        return None;
    }
    if message.role == MessageRole::Assistant && message.tool_calls_json.is_some() {
        return None;
    }
    Some(ChatMessage {
        role: match message.role {
            MessageRole::User => "user",
            MessageRole::Assistant => "assistant",
            MessageRole::System => "system",
            MessageRole::Tool => "tool",
        }
        .to_string(),
        content: ChatContent::Text(message.content.clone()),
        tool_calls: None,
        tool_call_id: None,
    })
}

fn sse_json(value: serde_json::Value) -> Result<Event, std::convert::Infallible> {
    Ok(Event::default().data(value.to_string()))
}

async fn bridge_message(
    AxumState(runtime): AxumState<BridgeRuntime>,
    Json(req): Json<BridgeMessageRequest>,
) -> Sse<Pin<Box<dyn Stream<Item = Result<Event, std::convert::Infallible>> + Send>>> {
    let (tx, rx) = mpsc::channel::<Result<Event, std::convert::Infallible>>(64);
    tokio::spawn(async move {
        let start = Instant::now();
        if let Err(error) = stream_project_conversation(runtime, req, tx.clone(), start).await {
            let _ = tx
                .send(sse_json(serde_json::json!({ "type": "result", "error": error })))
                .await;
        }
    });
    let stream: Pin<Box<dyn Stream<Item = Result<Event, std::convert::Infallible>> + Send>> =
        Box::pin(ReceiverStream::new(rx));
    Sse::new(stream).keep_alive(KeepAlive::default())
}

async fn stream_project_conversation(
    runtime: BridgeRuntime,
    req: BridgeMessageRequest,
    tx: mpsc::Sender<Result<Event, std::convert::Infallible>>,
    start: Instant,
) -> Result<(), String> {
    let assignment = normalize_im_assignment(req.assignment.clone()).unwrap_or_else(|| "aiagent".to_string());
    let prompt = match req.files.as_ref().filter(|files| !files.is_empty()) {
        Some(files) => format!("{}\n\n[IM attachments]\n{}", req.prompt, files.join("\n")),
        None => req.prompt.clone(),
    };
    let (conversation, provider, model) =
        get_or_create_session_conversation(&runtime, &req.session_key, &prompt).await?;

    let user_message = frogclaw_core::repo::message::create_message(
        &runtime.db,
        &conversation.id,
        MessageRole::User,
        &prompt,
        &[],
        None,
        0,
    )
    .await
    .map_err(|e| e.to_string())?;
    frogclaw_core::repo::conversation::increment_message_count(&runtime.db, &conversation.id)
        .await
        .map_err(|e| e.to_string())?;

    let messages = frogclaw_core::repo::message::list_messages(&runtime.db, &conversation.id)
        .await
        .map_err(|e| e.to_string())?;
    let mut chat_messages: Vec<ChatMessage> = messages.iter().filter_map(message_to_chat).collect();

    let settings = frogclaw_core::repo::settings::get_settings(&runtime.db).await.unwrap_or_default();
    if let Some(system_prompt) = settings
        .default_system_prompt
        .clone()
        .filter(|s| !s.trim().is_empty())
    {
        chat_messages.insert(
            0,
            ChatMessage {
                role: "system".to_string(),
                content: ChatContent::Text(system_prompt),
                tool_calls: None,
                tool_call_id: None,
            },
        );
    }

    let key_row = frogclaw_core::repo::provider::get_active_key(&runtime.db, &provider.id)
        .await
        .map_err(|e| e.to_string())?;
    let api_key = frogclaw_core::crypto::decrypt_key(&key_row.key_encrypted, &runtime.master_key)
        .map_err(|e| e.to_string())?;
    let resolved_proxy = ProviderProxyConfig::resolve(&provider.proxy_config, &settings);
    let ctx = ProviderRequestContext {
        api_key,
        key_id: key_row.id.clone(),
        provider_id: provider.id.clone(),
        base_url: Some(resolve_base_url_for_type(&provider.api_host, &provider.provider_type)),
        api_path: provider.api_path.clone(),
        proxy_config: resolved_proxy,
        custom_headers: provider.custom_headers.as_ref().and_then(|s| serde_json::from_str(s).ok()),
    };

    let registry = ProviderRegistry::create_default();
    let registry_key = provider_type_to_registry_key(&provider.provider_type);
    let adapter: &dyn ProviderAdapter = registry
        .get(registry_key)
        .ok_or_else(|| format!("Unsupported provider type: {}", registry_key))?;
    let model_params = model.param_overrides.clone();
    let request = ChatRequest {
        model: model.model_id.clone(),
        messages: chat_messages,
        stream: true,
        temperature: None,
        top_p: None,
        max_tokens: model_params.as_ref().and_then(|p| p.max_tokens),
        tools: None,
        thinking_budget: None,
        thinking_level: None,
        reasoning_profile: model_params.as_ref().and_then(|p| p.reasoning_profile.clone()),
        use_max_completion_tokens: model_params.as_ref().and_then(|p| p.use_max_completion_tokens),
        thinking_param_style: model_params.as_ref().and_then(|p| p.thinking_param_style.clone()),
    };
    let cancel_flag = Arc::new(AtomicBool::new(false));
    runtime
        .cancel_flags
        .lock()
        .await
        .insert(req.session_key.clone(), cancel_flag.clone());

    tx.send(sse_json(serde_json::json!({
        "type": "system",
        "model": model.model_id,
        "conversationId": conversation.id,
        "assignment": assignment,
    })))
    .await
    .ok();

    let mut stream = adapter.chat_stream(&ctx, request);
    let mut assistant_content = String::new();
    let mut usage: Option<TokenUsage> = None;
    let mut stream_error: Option<String> = None;

    while let Some(result) = stream.next().await {
        if cancel_flag.load(Ordering::Relaxed) {
            stream_error = Some("Cancelled".to_string());
            break;
        }
        match result {
            Ok(chunk) => {
                if let Some(delta) = chunk.thinking.or(chunk.content) {
                    if !delta.is_empty() {
                        assistant_content.push_str(&delta);
                        tx.send(sse_json(serde_json::json!({ "type": "text", "delta": delta })))
                            .await
                            .ok();
                    }
                }
                if chunk.usage.is_some() {
                    usage = chunk.usage;
                }
                if chunk.done {
                    break;
                }
            }
            Err(e) => {
                stream_error = Some(e.to_string());
                break;
            }
        }
    }

    let _assistant_message = frogclaw_core::repo::message::create_message(
        &runtime.db,
        &conversation.id,
        MessageRole::Assistant,
        &assistant_content,
        &[],
        Some(&user_message.id),
        0,
    )
    .await
    .map_err(|e| e.to_string())?;
    frogclaw_core::repo::conversation::increment_message_count(&runtime.db, &conversation.id)
        .await
        .map_err(|e| e.to_string())?;
    runtime.cancel_flags.lock().await.remove(&req.session_key);

    tx.send(sse_json(serde_json::json!({
        "type": "result",
        "error": stream_error,
        "totalTokens": usage.as_ref().map(|u| u.total_tokens),
        "durationMs": start.elapsed().as_millis() as u64,
        "model": model.model_id,
    })))
    .await
    .ok();
    Ok(())
}

async fn bridge_reset(
    AxumState(runtime): AxumState<BridgeRuntime>,
    Json(req): Json<SessionRequest>,
) -> Json<serde_json::Value> {
    runtime.sessions.lock().await.remove(&req.session_key);
    Json(serde_json::json!({ "ok": true }))
}

async fn bridge_cancel(
    AxumState(runtime): AxumState<BridgeRuntime>,
    Json(req): Json<SessionRequest>,
) -> Json<serde_json::Value> {
    if let Some(flag) = runtime.cancel_flags.lock().await.get(&req.session_key) {
        flag.store(true, Ordering::Relaxed);
    }
    Json(serde_json::json!({ "ok": true }))
}

#[tauri::command]
pub async fn get_im_channels() -> Result<Vec<ImChannel>, String> {
    Ok(read_channels_file()?.channels)
}

#[tauri::command]
pub async fn save_im_channels(channels: Vec<ImChannel>) -> Result<(), String> {
    write_channels_file(&ImChannelsFile { channels })
}

#[tauri::command]
pub async fn platform_status(bridge: State<'_, PlatformBridgeState>) -> Result<PlatformStatus, String> {
    let tracked_running = bridge.child.lock().await.as_mut().is_some_and(|child| {
        child.try_wait().ok().flatten().is_none()
    });
    let running = tracked_running || is_platform_port_open();
    Ok(PlatformStatus {
        running,
        parent_port: *bridge.parent_port.lock().await,
        config_path: channels_path()?.display().to_string(),
        log_path: log_path()?.display().to_string(),
    })
}

#[tauri::command]
pub async fn platform_start(
    app_state: State<'_, AppState>,
    bridge: State<'_, PlatformBridgeState>,
) -> Result<PlatformStatus, String> {
    if bridge.child.lock().await.as_mut().is_some_and(|child| {
        child.try_wait().ok().flatten().is_none()
    }) {
        return platform_status(bridge).await;
    }
    if is_platform_port_open() {
        let _ = request_platform_reload();
        return platform_status(bridge).await;
    }

    let port = start_parent_server(&app_state, &bridge).await?;
    let sidecar = sidecar_path()?;
    let log_file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path()?)
        .map_err(|e| e.to_string())?;
    let log_file_err = log_file.try_clone().map_err(|e| e.to_string())?;
    let mut command = Command::new("node");
    command
        .arg(sidecar)
        .arg("--parent")
        .arg(format!("http://127.0.0.1:{port}"))
        .stdout(Stdio::from(log_file))
        .stderr(Stdio::from(log_file_err));
    #[cfg(target_os = "windows")]
    command.creation_flags(0x08000000);
    let child = command.spawn()
        .map_err(|e| format!("Failed to start platform sidecar with node: {e}"))?;
    *bridge.child.lock().await = Some(child);
    platform_status(bridge).await
}

#[tauri::command]
pub async fn platform_stop(bridge: State<'_, PlatformBridgeState>) -> Result<PlatformStatus, String> {
    if let Some(mut child) = bridge.child.lock().await.take() {
        let _ = child.kill();
        let _ = child.wait();
    }
    platform_status(bridge).await
}

#[tauri::command]
pub async fn platform_reload_config(
    app_state: State<'_, AppState>,
    bridge: State<'_, PlatformBridgeState>,
) -> Result<PlatformStatus, String> {
    if let Some(mut child) = bridge.child.lock().await.take() {
        let _ = child.kill();
        let _ = child.wait();
    }
    if is_platform_port_open() && request_platform_reload() {
        return platform_status(bridge).await;
    }
    platform_start(app_state, bridge).await
}

#[tauri::command]
pub async fn platform_connect_feishu(
    app_state: State<'_, AppState>,
    bridge: State<'_, PlatformBridgeState>,
) -> Result<PlatformStatus, String> {
    platform_start(app_state, bridge).await
}

#[tauri::command]
pub async fn platform_read_log(max_bytes: Option<u64>) -> Result<String, String> {
    read_log_file(log_path()?, max_bytes)
}

fn read_log_file(path: PathBuf, max_bytes: Option<u64>) -> Result<String, String> {
    if !path.exists() {
        return Ok(String::new());
    }
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let max = max_bytes.unwrap_or(64 * 1024) as usize;
    let slice = if bytes.len() > max {
        &bytes[bytes.len() - max..]
    } else {
        &bytes[..]
    };
    Ok(String::from_utf8_lossy(slice).to_string())
}

#[tauri::command]
pub async fn install_read_log(max_bytes: Option<u64>) -> Result<String, String> {
    read_log_file(config_dir()?.join("install.log"), max_bytes)
}

#[tauri::command]
pub async fn codex_app_server_read_log(max_bytes: Option<u64>) -> Result<String, String> {
    read_log_file(config_dir()?.join("ai-agent.log"), max_bytes)
}

#[tauri::command]
pub async fn get_log_file_path(source: String) -> Result<String, String> {
    let file_name = match source.as_str() {
        "install" => "install.log",
        "sidecar" | "platform" => "platform-sidecar.log",
        "ai_agent" | "codex_app_server" | "codex" => "ai-agent.log",
        other => return Err(format!("Unknown log source: {other}")),
    };
    Ok(config_dir()?.join(file_name).display().to_string())
}
