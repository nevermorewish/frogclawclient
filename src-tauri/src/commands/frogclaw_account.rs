use crate::AppState;
use frogclaw_core::types::{
    DeepLinkProviderImportInput, Model, ModelCapability, ModelType, ProviderType, UpdateProviderInput,
};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;
use std::time::Duration;
use tauri::State;

const FROGCLAW_BASE_URL: &str = "https://frogclaw.com";
const FROGCLAW_PROVIDER_PREFIX: &str = "frogclaw-";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FrogclawUserData {
    pub id: i64,
    pub username: String,
    pub display_name: String,
    pub role: i64,
    pub status: i64,
    pub group: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FrogclawToken {
    pub id: i64,
    pub key: String,
    pub name: String,
    pub status: i64,
    pub remain_quota: i64,
    pub unlimited_quota: bool,
    #[serde(default)]
    pub group: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FrogclawSystemProvider {
    pub id: i64,
    pub name: String,
    pub provider_key: String,
    pub api_mode: String,
    pub needs_v1_suffix: bool,
    pub base_url: String,
    pub default_model: Option<String>,
    pub use_site_token: bool,
    pub token_group: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FrogclawCliProvider {
    pub id: i64,
    pub name: String,
    pub provider_type: String,
    pub base_url: Option<String>,
    pub api_key: Option<String>,
    pub settings_config: Option<String>,
    pub is_default: Option<bool>,
    pub created_time: Option<i64>,
    pub updated_time: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenClawModelInfo {
    pub id: String,
    pub name: String,
    pub provider: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct FrogclawLoginSession {
    pub user: FrogclawUserData,
    pub tokens: Vec<FrogclawToken>,
    pub system_providers: Vec<FrogclawSystemProvider>,
    pub cli_providers: Vec<FrogclawCliProvider>,
}

#[derive(Debug, Serialize)]
pub struct FrogclawConfiguredProvider {
    pub provider_id: String,
    pub name: String,
    pub provider_type: String,
    pub model_id: Option<String>,
    pub token_name: String,
    pub token_group: String,
    pub created_provider: bool,
    pub added_key: bool,
    pub reused_key: bool,
}

#[derive(Debug, Serialize)]
pub struct OpenClawConfigSummary {
    pub applied: bool,
    pub path: Option<String>,
    pub models: Vec<OpenClawModelInfo>,
}

#[derive(Debug, Serialize)]
pub struct FrogclawConfigureResult {
    pub session: FrogclawLoginSession,
    pub configured_providers: Vec<FrogclawConfiguredProvider>,
    pub openclaw: OpenClawConfigSummary,
}

#[derive(Debug, Deserialize)]
struct FrogclawResponse<T> {
    success: bool,
    message: String,
    data: Option<T>,
}

#[derive(Debug, Serialize)]
struct LoginBody {
    username: String,
    password: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct TokenListResponse {
    items: Option<Vec<FrogclawToken>>,
}

#[derive(Debug, Serialize, Deserialize)]
struct CliProviderListResponse {
    items: Option<Vec<FrogclawCliProvider>>,
}

fn account_log(event: &str, detail: &str) {
    let path = crate::paths::frogclaw_home().join("frogclaw-account.log");
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        use std::io::Write;
        let ts = chrono::Local::now().format("%Y-%m-%dT%H:%M:%S%.3f");
        let _ = writeln!(f, "[{}] [{}] {}", ts, event, detail);
    }
}

fn normalize_group(value: &str) -> String {
    value.to_lowercase().split_whitespace().collect::<String>()
}

fn token_matches_group(token: &FrogclawToken, group: &str) -> bool {
    let token_group = normalize_group(&token.group);
    let required = normalize_group(group);
    token_group == required || (required == "default" && token_group.is_empty())
}

fn provider_type_for_key(provider_key: &str, api_mode: &str) -> Option<ProviderType> {
    match provider_key {
        "anthropic" | "claude" => Some(ProviderType::Anthropic),
        "google" | "gemini" => Some(ProviderType::Gemini),
        "openai" | "codex" => {
            if api_mode == "chat" {
                Some(ProviderType::OpenAI)
            } else {
                Some(ProviderType::OpenAIResponses)
            }
        }
        _ => None,
    }
}

fn provider_type_name(provider_type: &ProviderType) -> &'static str {
    match provider_type {
        ProviderType::OpenAI => "openai",
        ProviderType::OpenAIResponses => "openai_responses",
        ProviderType::Anthropic => "anthropic",
        ProviderType::Gemini => "gemini",
        ProviderType::Jina => "jina",
        ProviderType::Cohere => "cohere",
        ProviderType::Voyage => "voyage",
        ProviderType::Custom => "custom",
    }
}

fn base_url_for_provider(sp: &FrogclawSystemProvider, provider_type: &ProviderType) -> String {
    let raw = sp.base_url.trim();
    let fallback = match provider_type {
        ProviderType::OpenAI | ProviderType::OpenAIResponses => "https://frogclaw.com/v1",
        _ => "https://frogclaw.com",
    };
    let base = if raw.is_empty() { fallback } else { raw };
    let base = base.trim_end_matches('/').to_string();
    if sp.needs_v1_suffix && !base.ends_with("/v1") {
        format!("{base}/v1")
    } else {
        base
    }
}

fn selected_token_for_provider<'a>(
    sp: &FrogclawSystemProvider,
    tokens: &'a [FrogclawToken],
) -> Option<&'a FrogclawToken> {
    if !sp.token_group.trim().is_empty() {
        if let Some(token) = tokens.iter().find(|t| token_matches_group(t, &sp.token_group)) {
            return Some(token);
        }
    }
    if matches!(sp.provider_key.as_str(), "openai" | "codex" | "google" | "gemini") {
        if let Some(token) = tokens.iter().find(|t| token_matches_group(t, "default")) {
            return Some(token);
        }
    }
    tokens.first()
}

async fn login_and_client(username: &str, password: &str) -> Result<(Client, FrogclawUserData), String> {
    let client = Client::builder()
        .timeout(Duration::from_secs(20))
        .cookie_store(true)
        .cookie_provider(Arc::new(reqwest::cookie::Jar::default()))
        .build()
        .map_err(|e| format!("Failed to create HTTP client: {e}"))?;

    let resp = client
        .post(format!("{FROGCLAW_BASE_URL}/api/user/login"))
        .json(&LoginBody {
            username: username.to_string(),
            password: password.to_string(),
        })
        .send()
        .await
        .map_err(|e| format!("Login request failed: {e}"))?;

    if !resp.status().is_success() {
        return Err(format!("Server error: {}", resp.status()));
    }

    let result: FrogclawResponse<FrogclawUserData> = resp
        .json()
        .await
        .map_err(|e| format!("Failed to parse login response: {e}"))?;
    if !result.success {
        return Err(result.message);
    }
    let user = result
        .data
        .ok_or_else(|| "Login succeeded but no user data returned".to_string())?;
    Ok((client, user))
}

async fn fetch_session_data(client: &Client, user: FrogclawUserData) -> Result<FrogclawLoginSession, String> {
    let user_id = user.id.to_string();

    let tokens = match client
        .get(format!("{FROGCLAW_BASE_URL}/api/token/?p=0&size=100"))
        .header("New-Api-User", &user_id)
        .send()
        .await
    {
        Ok(resp) if resp.status().is_success() => {
            let result: FrogclawResponse<TokenListResponse> = resp.json().await.unwrap_or(FrogclawResponse {
                success: false,
                message: String::new(),
                data: None,
            });
            if result.success {
                result
                    .data
                    .and_then(|d| d.items)
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|t| t.status == 1)
                    .collect()
            } else {
                Vec::new()
            }
        }
        _ => Vec::new(),
    };

    let system_providers = match client
        .get(format!("{FROGCLAW_BASE_URL}/api/system-cli-provider/"))
        .header("New-Api-User", &user_id)
        .send()
        .await
    {
        Ok(resp) if resp.status().is_success() => {
            let result: FrogclawResponse<Vec<FrogclawSystemProvider>> = resp.json().await.unwrap_or(FrogclawResponse {
                success: false,
                message: String::new(),
                data: None,
            });
            if result.success {
                result.data.unwrap_or_default()
            } else {
                Vec::new()
            }
        }
        _ => Vec::new(),
    };

    let cli_providers = match client
        .get(format!("{FROGCLAW_BASE_URL}/api/cli-provider/?p=0&page_size=100"))
        .header("New-Api-User", &user_id)
        .send()
        .await
    {
        Ok(resp) if resp.status().is_success() => {
            let result: FrogclawResponse<CliProviderListResponse> = resp.json().await.unwrap_or(FrogclawResponse {
                success: false,
                message: String::new(),
                data: None,
            });
            if result.success {
                result.data.and_then(|d| d.items).unwrap_or_default()
            } else {
                Vec::new()
            }
        }
        _ => Vec::new(),
    };

    Ok(FrogclawLoginSession {
        user,
        tokens,
        system_providers,
        cli_providers,
    })
}

async fn ensure_group_token(client: &Client, user_id: i64, group: &str) -> Result<(), String> {
    #[derive(Serialize)]
    struct EnsureBody<'a> {
        group: &'a str,
    }

    let resp = client
        .post(format!("{FROGCLAW_BASE_URL}/api/token/ensure-group"))
        .header("New-Api-User", user_id.to_string())
        .json(&EnsureBody { group })
        .send()
        .await
        .map_err(|e| format!("ensure-group request failed: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("ensure-group server error: {}", resp.status()));
    }
    let result: FrogclawResponse<Value> = resp
        .json()
        .await
        .map_err(|e| format!("Failed to parse ensure-group response: {e}"))?;
    if !result.success {
        return Err(result.message);
    }
    Ok(())
}

async fn ensure_required_token_groups(
    client: &Client,
    session: &FrogclawLoginSession,
) -> Result<bool, String> {
    let mut changed = false;
    let needs_default = session.system_providers.iter().any(|sp| {
        matches!(sp.provider_key.as_str(), "openai" | "codex" | "google" | "gemini")
    }) || session.cli_providers.iter().any(|p| p.provider_type == "openclaw");
    if needs_default && !session.tokens.iter().any(|t| token_matches_group(t, "default")) {
        ensure_group_token(client, session.user.id, "default").await?;
        changed = true;
    }
    if session
        .system_providers
        .iter()
        .any(|sp| matches!(sp.provider_key.as_str(), "anthropic" | "claude"))
        && !session.tokens.iter().any(|t| token_matches_group(t, "Claude Max"))
    {
        let _ = ensure_group_token(client, session.user.id, "Claude Max").await;
        changed = true;
    }
    Ok(changed)
}

fn extract_openclaw_config(session: &FrogclawLoginSession) -> (Option<String>, Vec<OpenClawModelInfo>) {
    let mut candidates: Vec<&FrogclawCliProvider> = session
        .cli_providers
        .iter()
        .filter(|p| p.provider_type == "openclaw")
        .collect();
    candidates.sort_by_key(|p| std::cmp::Reverse(p.updated_time.unwrap_or_default()));
    let chosen = candidates
        .iter()
        .copied()
        .find(|p| p.is_default.unwrap_or(false))
        .or_else(|| candidates.first().copied());

    let Some(config_json) = chosen.and_then(|p| p.settings_config.clone()) else {
        return (None, Vec::new());
    };
    let Ok(config) = serde_json::from_str::<Value>(&config_json) else {
        return (Some(config_json), Vec::new());
    };
    let mut models = Vec::new();
    if let Some(providers) = config.pointer("/models/providers").and_then(|v| v.as_object()) {
        for (provider, data) in providers {
            if let Some(model_list) = data.get("models").and_then(|v| v.as_array()) {
                for model in model_list {
                    if let Some(id) = model.get("id").and_then(|v| v.as_str()) {
                        models.push(OpenClawModelInfo {
                            id: id.to_string(),
                            name: model
                                .get("name")
                                .and_then(|v| v.as_str())
                                .unwrap_or(id)
                                .to_string(),
                            provider: provider.clone(),
                        });
                    }
                }
            }
        }
    }
    (Some(config_json), models)
}

fn apply_openclaw_config_to_disk(config_json: &str) -> Result<String, String> {
    let server_config: Value = serde_json::from_str(config_json).map_err(|e| format!("Invalid JSON: {e}"))?;
    let config_dir = crate::paths::frogclaw_home().join("openclaw").join("config");
    let config_path = config_dir.join("openclaw.json");
    std::fs::create_dir_all(&config_dir).map_err(|e| format!("Failed to create config dir: {e}"))?;

    let mut existing: Value = if config_path.exists() {
        std::fs::read_to_string(&config_path)
            .ok()
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_else(|| Value::Object(serde_json::Map::new()))
    } else {
        Value::Object(serde_json::Map::new())
    };

    let saved_gateway_port = existing.pointer("/gateway/port").cloned();
    let saved_workspace = existing.pointer("/agents/defaults/workspace").cloned();

    if let (Some(existing_obj), Some(server_obj)) = (existing.as_object_mut(), server_config.as_object()) {
        for (key, value) in server_obj {
            existing_obj.insert(key.clone(), value.clone());
        }
    }

    if let Some(port) = saved_gateway_port {
        if let Some(obj) = existing.as_object_mut() {
            let gateway = obj.entry("gateway").or_insert_with(|| Value::Object(serde_json::Map::new()));
            if let Some(gateway_obj) = gateway.as_object_mut() {
                gateway_obj.insert("port".to_string(), port);
            }
        }
    }
    if let Some(workspace) = saved_workspace {
        if let Some(obj) = existing.as_object_mut() {
            let agents = obj.entry("agents").or_insert_with(|| Value::Object(serde_json::Map::new()));
            if let Some(agents_obj) = agents.as_object_mut() {
                let defaults = agents_obj
                    .entry("defaults")
                    .or_insert_with(|| Value::Object(serde_json::Map::new()));
                if let Some(defaults_obj) = defaults.as_object_mut() {
                    defaults_obj.insert("workspace".to_string(), workspace);
                }
            }
        }
    }

    let tmp_path = config_dir.join(format!(
        "openclaw.json.tmp.{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
    ));
    let raw = serde_json::to_string_pretty(&existing).map_err(|e| format!("Failed to serialize config: {e}"))?;
    std::fs::write(&tmp_path, raw).map_err(|e| format!("Failed to write temp config: {e}"))?;
    if cfg!(windows) && config_path.exists() {
        let _ = std::fs::remove_file(&config_path);
    }
    std::fs::rename(&tmp_path, &config_path).map_err(|e| format!("Failed to rename config: {e}"))?;
    Ok(config_path.to_string_lossy().to_string())
}

async fn configure_system_providers(
    state: &AppState,
    session: &FrogclawLoginSession,
) -> Result<Vec<FrogclawConfiguredProvider>, String> {
    let mut configured = Vec::new();

    for sp in &session.system_providers {
        let Some(provider_type) = provider_type_for_key(&sp.provider_key, &sp.api_mode) else {
            continue;
        };
        let Some(token) = selected_token_for_provider(sp, &session.tokens) else {
            continue;
        };

        let provider_name = format!("{FROGCLAW_PROVIDER_PREFIX}{}", sp.name);
        let api_key = format!("sk-{}", token.key);
        let base_url = base_url_for_provider(sp, &provider_type);
        let import_result = frogclaw_core::repo::provider::import_provider_from_deep_link(
            &state.sea_db,
            &state.master_key,
            DeepLinkProviderImportInput {
                name: provider_name.clone(),
                baseurl: base_url.clone(),
                apikey: api_key,
                provider_type: provider_type_name(&provider_type).to_string(),
            },
        )
        .await
        .map_err(|e| e.to_string())?;

        let provider = frogclaw_core::repo::provider::get_provider(&state.sea_db, &import_result.provider_id)
            .await
            .map_err(|e| e.to_string())?;

        let mut update = UpdateProviderInput::default();
        update.name = Some(provider_name.clone());
        update.provider_type = Some(provider_type.clone());
        update.api_host = Some(base_url);
        update.enabled = Some(true);
        let _ = frogclaw_core::repo::provider::update_provider(&state.sea_db, &provider.id, update).await;

        let model_id = sp.default_model.as_ref().filter(|m| !m.trim().is_empty()).cloned();
        if let Some(model_id) = &model_id {
            let capabilities = match provider_type {
                ProviderType::Anthropic | ProviderType::OpenAI | ProviderType::OpenAIResponses | ProviderType::Gemini => {
                    vec![
                        ModelCapability::TextChat,
                        ModelCapability::Vision,
                        ModelCapability::FunctionCalling,
                    ]
                }
                _ => vec![ModelCapability::TextChat],
            };
            let model = Model {
                provider_id: provider.id.clone(),
                model_id: model_id.clone(),
                name: model_id.clone(),
                group_name: Some("FrogClaw".into()),
                model_type: ModelType::Chat,
                capabilities,
                max_tokens: None,
                enabled: true,
                param_overrides: None,
            };
            frogclaw_core::repo::provider::save_models(&state.sea_db, &provider.id, &[model])
                .await
                .map_err(|e| e.to_string())?;
        }

        configured.push(FrogclawConfiguredProvider {
            provider_id: provider.id,
            name: provider_name,
            provider_type: provider_type_name(&provider_type).to_string(),
            model_id,
            token_name: token.name.clone(),
            token_group: if token.group.is_empty() {
                "default".into()
            } else {
                token.group.clone()
            },
            created_provider: import_result.created_provider,
            added_key: import_result.added_key,
            reused_key: import_result.reused_key,
        });
    }

    if configured.is_empty() && !session.tokens.is_empty() {
        let token = &session.tokens[0];
        let api_key = format!("sk-{}", token.key);
        let result = frogclaw_core::repo::provider::import_provider_from_deep_link(
            &state.sea_db,
            &state.master_key,
            DeepLinkProviderImportInput {
                name: "frogclaw-openai".into(),
                baseurl: "https://frogclaw.com/v1".into(),
                apikey: api_key,
                provider_type: "openai_responses".into(),
            },
        )
        .await
        .map_err(|e| e.to_string())?;
        configured.push(FrogclawConfiguredProvider {
            provider_id: result.provider_id,
            name: result.provider_name,
            provider_type: "openai_responses".into(),
            model_id: None,
            token_name: token.name.clone(),
            token_group: if token.group.is_empty() { "default".into() } else { token.group.clone() },
            created_provider: result.created_provider,
            added_key: result.added_key,
            reused_key: result.reused_key,
        });
    }

    Ok(configured)
}

#[tauri::command]
pub async fn fetch_and_configure_frogclaw(
    state: State<'_, AppState>,
    username: String,
    password: String,
) -> Result<FrogclawConfigureResult, String> {
    let (client, user) = login_and_client(&username, &password).await?;
    let mut session = fetch_session_data(&client, user).await?;
    if ensure_required_token_groups(&client, &session).await.unwrap_or(false) {
        session = fetch_session_data(&client, session.user.clone()).await?;
    }

    account_log(
        "login",
        &format!(
            "user={} tokens={} system_providers={} cli_providers={}",
            session.user.username,
            session.tokens.len(),
            session.system_providers.len(),
            session.cli_providers.len()
        ),
    );

    let configured_providers = configure_system_providers(&state, &session).await?;
    let (openclaw_config_json, openclaw_models) = extract_openclaw_config(&session);
    let openclaw = if let Some(config_json) = openclaw_config_json {
        match apply_openclaw_config_to_disk(&config_json) {
            Ok(path) => OpenClawConfigSummary {
                applied: true,
                path: Some(path),
                models: openclaw_models,
            },
            Err(e) => {
                account_log("openclaw-config-error", &e);
                OpenClawConfigSummary {
                    applied: false,
                    path: None,
                    models: openclaw_models,
                }
            }
        }
    } else {
        OpenClawConfigSummary {
            applied: false,
            path: None,
            models: openclaw_models,
        }
    };

    Ok(FrogclawConfigureResult {
        session,
        configured_providers,
        openclaw,
    })
}

#[tauri::command]
pub async fn apply_openclaw_config(config_json: String) -> Result<String, String> {
    apply_openclaw_config_to_disk(&config_json)
}
