use crate::frogclaw_config::FROGCLAW_BASE_URL;
use crate::AppState;
use frogclaw_core::crypto::encrypt_key;
use frogclaw_core::types::{
    CreateProviderInput, Model, ModelCapability, ModelType, ProviderType, UpdateProviderInput,
};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;
use std::time::Duration;
use tauri::State;

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
pub struct FrogclawPricingModel {
    pub model_name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub tags: String,
    #[serde(default)]
    pub vendor_id: Option<i64>,
    #[serde(default)]
    pub enable_groups: Vec<String>,
    #[serde(default)]
    pub supported_endpoint_types: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FrogclawPricingVendor {
    pub id: i64,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub icon: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct FrogclawLoginSession {
    pub user: FrogclawUserData,
    pub tokens: Vec<FrogclawToken>,
    pub system_providers: Vec<FrogclawSystemProvider>,
    pub pricing_models: Vec<FrogclawPricingModel>,
    pub pricing_vendors: Vec<FrogclawPricingVendor>,
}

#[derive(Debug, Serialize)]
pub struct FrogclawConfiguredProvider {
    pub provider_id: String,
    pub name: String,
    pub provider_type: String,
    pub model_count: usize,
    pub token_id: i64,
    pub token_name: String,
    pub token_group: String,
    pub created_provider: bool,
    pub updated_key: bool,
}

#[derive(Debug, Serialize)]
pub struct FrogclawConfigureResult {
    pub session: FrogclawLoginSession,
    pub configured_providers: Vec<FrogclawConfiguredProvider>,
    pub selected_token_id: Option<i64>,
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

#[derive(Debug, Deserialize)]
struct PricingResponse {
    success: bool,
    #[serde(default)]
    data: Vec<FrogclawPricingModel>,
    #[serde(default)]
    vendors: Vec<FrogclawPricingVendor>,
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

fn token_group(token: &FrogclawToken) -> String {
    let group = token.group.trim();
    if group.is_empty() {
        "default".to_string()
    } else {
        group.to_string()
    }
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

fn model_supports_provider(model: &FrogclawPricingModel, provider_type: &ProviderType) -> bool {
    if model.supported_endpoint_types.is_empty() {
        return true;
    }
    model.supported_endpoint_types.iter().any(|endpoint| match provider_type {
        ProviderType::Anthropic => endpoint == "anthropic",
        ProviderType::Gemini => endpoint == "gemini",
        ProviderType::OpenAI | ProviderType::OpenAIResponses => {
            endpoint == "openai" || endpoint == "chat_completions" || endpoint == "responses"
        }
        _ => true,
    })
}

fn capabilities_for_model(model_id: &str, provider_type: &ProviderType) -> Vec<ModelCapability> {
    let mut capabilities = vec![ModelCapability::TextChat];
    let lower = model_id.to_lowercase();
    if matches!(
        provider_type,
        ProviderType::Anthropic | ProviderType::OpenAI | ProviderType::OpenAIResponses | ProviderType::Gemini
    ) {
        capabilities.push(ModelCapability::Vision);
        capabilities.push(ModelCapability::FunctionCalling);
    }
    if lower.contains("reason") || lower.contains("thinking") || lower.starts_with('o') {
        capabilities.push(ModelCapability::Reasoning);
    }
    capabilities
}

async fn login_and_client(username: &str, password: &str) -> Result<(Client, FrogclawUserData), String> {
    let client = Client::builder()
        .timeout(Duration::from_secs(20))
        .cookie_store(true)
        .cookie_provider(Arc::new(reqwest::cookie::Jar::default()))
        .build()
        .map_err(|e| format!("创建 HTTP 客户端失败: {e}"))?;

    let resp = client
        .post(format!("{FROGCLAW_BASE_URL}/api/user/login"))
        .json(&LoginBody {
            username: username.to_string(),
            password: password.to_string(),
        })
        .send()
        .await
        .map_err(|e| format!("登录请求失败: {e}"))?;

    if !resp.status().is_success() {
        return Err(format!("服务器错误: {}", resp.status()));
    }

    let result: FrogclawResponse<FrogclawUserData> = resp
        .json()
        .await
        .map_err(|e| format!("解析登录响应失败: {e}"))?;
    if !result.success {
        return Err(result.message);
    }
    result
        .data
        .ok_or_else(|| "登录成功，但服务器没有返回用户信息".to_string())
        .map(|user| (client, user))
}

async fn fetch_tokens(client: &Client, user_id: i64) -> Vec<FrogclawToken> {
    match client
        .get(format!("{FROGCLAW_BASE_URL}/api/token/?p=0&size=100"))
        .header("New-Api-User", user_id.to_string())
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
    }
}

async fn fetch_system_providers(client: &Client, user_id: i64) -> Vec<FrogclawSystemProvider> {
    match client
        .get(format!("{FROGCLAW_BASE_URL}/api/system-cli-provider/"))
        .header("New-Api-User", user_id.to_string())
        .send()
        .await
    {
        Ok(resp) if resp.status().is_success() => {
            let result: FrogclawResponse<Vec<FrogclawSystemProvider>> =
                resp.json().await.unwrap_or(FrogclawResponse {
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
    }
}

async fn fetch_pricing(client: &Client, user_id: i64) -> (Vec<FrogclawPricingModel>, Vec<FrogclawPricingVendor>) {
    match client
        .get(format!("{FROGCLAW_BASE_URL}/api/pricing"))
        .header("New-Api-User", user_id.to_string())
        .send()
        .await
    {
        Ok(resp) if resp.status().is_success() => {
            let pricing: PricingResponse = resp.json().await.unwrap_or(PricingResponse {
                success: false,
                data: Vec::new(),
                vendors: Vec::new(),
            });
            if pricing.success {
                (pricing.data, pricing.vendors)
            } else {
                (Vec::new(), Vec::new())
            }
        }
        _ => (Vec::new(), Vec::new()),
    }
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
        .map_err(|e| format!("ensure-group 请求失败: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("ensure-group 服务器错误: {}", resp.status()));
    }
    let result: FrogclawResponse<Value> = resp
        .json()
        .await
        .map_err(|e| format!("解析 ensure-group 响应失败: {e}"))?;
    if !result.success {
        return Err(result.message);
    }
    Ok(())
}

async fn fetch_session_data(client: &Client, user: FrogclawUserData) -> Result<FrogclawLoginSession, String> {
    let tokens = fetch_tokens(client, user.id).await;
    let system_providers = fetch_system_providers(client, user.id).await;
    let (pricing_models, pricing_vendors) = fetch_pricing(client, user.id).await;

    Ok(FrogclawLoginSession {
        user,
        tokens,
        system_providers,
        pricing_models,
        pricing_vendors,
    })
}

async fn ensure_required_token_groups(client: &Client, session: &FrogclawLoginSession) -> Result<bool, String> {
    let mut changed = false;
    if session
        .system_providers
        .iter()
        .any(|sp| matches!(sp.provider_key.as_str(), "openai" | "codex" | "google" | "gemini"))
        && !session.tokens.iter().any(|t| token_matches_group(t, "default"))
    {
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

fn selected_token<'a>(tokens: &'a [FrogclawToken], selected_token_id: Option<i64>) -> Option<&'a FrogclawToken> {
    selected_token_id
        .and_then(|id| tokens.iter().find(|token| token.id == id))
        .or_else(|| tokens.iter().find(|token| token_matches_group(token, "default")))
        .or_else(|| tokens.first())
}

fn key_prefix(raw_key: &str) -> String {
    if raw_key.len() >= 8 {
        format!("{}...", &raw_key[..8])
    } else {
        raw_key.to_string()
    }
}

async fn ensure_provider_with_token(
    state: &AppState,
    name: &str,
    provider_type: ProviderType,
    raw_key: &str,
) -> Result<(String, bool, bool), String> {
    let providers = frogclaw_core::repo::provider::list_providers(&state.sea_db)
        .await
        .map_err(|e| e.to_string())?;
    let (provider, created_provider) = if let Some(provider) = providers
        .into_iter()
        .find(|p| p.provider_type == provider_type && p.api_host.trim_end_matches('/') == FROGCLAW_BASE_URL)
    {
        (provider, false)
    } else {
        (
            frogclaw_core::repo::provider::create_provider(
                &state.sea_db,
                CreateProviderInput {
                    name: name.to_string(),
                    provider_type: provider_type.clone(),
                    api_host: FROGCLAW_BASE_URL.to_string(),
                    api_path: None,
                    enabled: true,
                    builtin_id: None,
                },
            )
            .await
            .map_err(|e| e.to_string())?,
            true,
        )
    };

    let mut update = UpdateProviderInput::default();
    update.name = Some(name.to_string());
    update.provider_type = Some(provider_type);
    update.api_host = Some(FROGCLAW_BASE_URL.to_string());
    update.enabled = Some(true);
    let provider = frogclaw_core::repo::provider::update_provider(&state.sea_db, &provider.id, update)
        .await
        .map_err(|e| e.to_string())?;

    let encrypted = encrypt_key(raw_key, &state.master_key).map_err(|e| e.to_string())?;
    let prefix = key_prefix(raw_key);
    let keys = frogclaw_core::repo::provider::list_keys_for_provider(&state.sea_db, &provider.id)
        .await
        .map_err(|e| e.to_string())?;
    if keys.is_empty() {
        frogclaw_core::repo::provider::add_provider_key(&state.sea_db, &provider.id, &encrypted, &prefix)
            .await
            .map_err(|e| e.to_string())?;
    } else {
        for key in keys {
            frogclaw_core::repo::provider::update_provider_key(&state.sea_db, &key.id, &encrypted, &prefix)
                .await
                .map_err(|e| e.to_string())?;
            if !key.enabled {
                let _ = frogclaw_core::repo::provider::toggle_provider_key(&state.sea_db, &key.id, true).await;
            }
        }
    }

    Ok((provider.id, created_provider, true))
}

async fn configure_system_providers(
    state: &AppState,
    session: &FrogclawLoginSession,
    selected_token_id: Option<i64>,
) -> Result<Vec<FrogclawConfiguredProvider>, String> {
    let Some(token) = selected_token(&session.tokens, selected_token_id) else {
        return Ok(Vec::new());
    };
    let api_key = format!("sk-{}", token.key);
    let available_models: Vec<&FrogclawPricingModel> = session.pricing_models.iter().collect();

    let mut configured = Vec::new();
    for sp in &session.system_providers {
        let Some(provider_type) = provider_type_for_key(&sp.provider_key, &sp.api_mode) else {
            continue;
        };
        let provider_name = format!("{FROGCLAW_PROVIDER_PREFIX}{}", sp.name);
        let (provider_id, created_provider, updated_key) =
            ensure_provider_with_token(state, &provider_name, provider_type.clone(), &api_key).await?;
        let models: Vec<Model> = available_models
            .iter()
            .filter(|model| model_supports_provider(model, &provider_type))
            .map(|model| Model {
                provider_id: provider_id.clone(),
                model_id: model.model_name.clone(),
                name: model.model_name.clone(),
                group_name: Some("FrogClaw".into()),
                model_type: ModelType::detect(&model.model_name),
                capabilities: capabilities_for_model(&model.model_name, &provider_type),
                max_tokens: None,
                enabled: true,
                param_overrides: None,
            })
            .collect();
        frogclaw_core::repo::provider::save_models(&state.sea_db, &provider_id, &models)
            .await
            .map_err(|e| e.to_string())?;

        configured.push(FrogclawConfiguredProvider {
            provider_id,
            name: provider_name,
            provider_type: provider_type_name(&provider_type).to_string(),
            model_count: models.len(),
            token_id: token.id,
            token_name: token.name.clone(),
            token_group: token_group(token),
            created_provider,
            updated_key,
        });
    }

    if configured.is_empty() {
        let (provider_id, created_provider, updated_key) = ensure_provider_with_token(
            state,
            "frogclaw-openai",
            ProviderType::OpenAIResponses,
            &api_key,
        )
        .await?;
        let models: Vec<Model> = available_models
            .iter()
            .map(|model| Model {
                provider_id: provider_id.clone(),
                model_id: model.model_name.clone(),
                name: model.model_name.clone(),
                group_name: Some("FrogClaw".into()),
                model_type: ModelType::detect(&model.model_name),
                capabilities: capabilities_for_model(&model.model_name, &ProviderType::OpenAIResponses),
                max_tokens: None,
                enabled: true,
                param_overrides: None,
            })
            .collect();
        frogclaw_core::repo::provider::save_models(&state.sea_db, &provider_id, &models)
            .await
            .map_err(|e| e.to_string())?;
        configured.push(FrogclawConfiguredProvider {
            provider_id,
            name: "frogclaw-openai".into(),
            provider_type: "openai_responses".into(),
            model_count: models.len(),
            token_id: token.id,
            token_name: token.name.clone(),
            token_group: token_group(token),
            created_provider,
            updated_key,
        });
    }

    Ok(configured)
}

#[tauri::command]
pub async fn fetch_and_configure_frogclaw(
    state: State<'_, AppState>,
    username: String,
    password: String,
    selected_token_id: Option<i64>,
) -> Result<FrogclawConfigureResult, String> {
    let (client, user) = login_and_client(&username, &password).await?;
    let mut session = fetch_session_data(&client, user).await?;
    if ensure_required_token_groups(&client, &session).await.unwrap_or(false) {
        session = fetch_session_data(&client, session.user.clone()).await?;
    }

    let selected = selected_token(&session.tokens, selected_token_id).map(|t| t.id);
    account_log(
        "login",
        &format!(
            "user={} tokens={} system_providers={} pricing_models={} selected_token={:?}",
            session.user.username,
            session.tokens.len(),
            session.system_providers.len(),
            session.pricing_models.len(),
            selected
        ),
    );

    let configured_providers = configure_system_providers(&state, &session, selected).await?;
    Ok(FrogclawConfigureResult {
        session,
        configured_providers,
        selected_token_id: selected,
    })
}

#[tauri::command]
pub async fn apply_frogclaw_token_selection(
    state: State<'_, AppState>,
    session: FrogclawLoginSession,
    selected_token_id: i64,
) -> Result<Vec<FrogclawConfiguredProvider>, String> {
    configure_system_providers(&state, &session, Some(selected_token_id)).await
}
