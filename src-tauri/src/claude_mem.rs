use frogclaw_core::types::{
    AppSettings, MemoryItem, MemoryNamespace, ModelType, ProjectMemoryProfile, ProviderConfig,
    ProviderType, RagContextResult, RagRetrievedItem, RagSourceResult,
};
use reqwest::Client;
use sea_orm::DatabaseConnection;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::OnceLock;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

const DEFAULT_BASE_URL: &str = "http://127.0.0.1:37777";
const DEFAULT_NAMESPACE_ID: &str = "claude-mem";
const DEFAULT_NAMESPACE_NAME: &str = "Claude-Mem";
const DEFAULT_CLAUDE_MEM_MODEL: &str = "claude-haiku-4-5-20251001";
const DEFAULT_GEMINI_MEM_MODEL: &str = "gemini-2.5-flash-lite";
const CLAUDE_MEM_WORKER_HOST: &str = "127.0.0.1";
const CLAUDE_MEM_WORKER_PORT: &str = "37777";
static START_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
static WORKER_PROCESS: OnceLock<Mutex<Option<Child>>> = OnceLock::new();
static RESOURCE_DIR: OnceLock<PathBuf> = OnceLock::new();
static RUNTIME_CONFIG: OnceLock<Mutex<Option<ClaudeMemRuntimeConfig>>> = OnceLock::new();
static SHUTTING_DOWN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

#[derive(Debug, Clone)]
struct ClaudeMemRuntimeConfig {
    provider: ClaudeMemProviderConfig,
    settings: BTreeMap<String, String>,
    env_file: BTreeMap<String, String>,
    process_env: BTreeMap<String, String>,
}

#[derive(Debug, Clone)]
enum ClaudeMemProviderConfig {
    ClaudeGateway {
        provider_id: String,
        model_id: String,
    },
    Gemini {
        provider_id: String,
        model_id: String,
    },
    OpenRouter {
        provider_id: String,
        model_id: String,
    },
    DefaultClaude,
}

impl ClaudeMemProviderConfig {
    fn label(&self) -> &'static str {
        match self {
            ClaudeMemProviderConfig::ClaudeGateway { .. } => "claude_gateway",
            ClaudeMemProviderConfig::Gemini { .. } => "gemini",
            ClaudeMemProviderConfig::OpenRouter { .. } => "openrouter",
            ClaudeMemProviderConfig::DefaultClaude => "default_claude",
        }
    }

    fn provider_id(&self) -> Option<&str> {
        match self {
            ClaudeMemProviderConfig::ClaudeGateway { provider_id, .. }
            | ClaudeMemProviderConfig::Gemini { provider_id, .. }
            | ClaudeMemProviderConfig::OpenRouter { provider_id, .. } => Some(provider_id),
            ClaudeMemProviderConfig::DefaultClaude => None,
        }
    }

    fn model_id(&self) -> Option<&str> {
        match self {
            ClaudeMemProviderConfig::ClaudeGateway { model_id, .. }
            | ClaudeMemProviderConfig::Gemini { model_id, .. }
            | ClaudeMemProviderConfig::OpenRouter { model_id, .. } => Some(model_id),
            ClaudeMemProviderConfig::DefaultClaude => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ClaudeMemClient {
    client: Client,
    base_url: String,
}

#[derive(Debug, Clone)]
pub struct ClaudeMemSaveInput {
    pub title: Option<String>,
    pub text: String,
    pub project: Option<String>,
    pub metadata: Option<Value>,
}

#[derive(Debug, Clone, Deserialize)]
struct SaveMemoryResponse {
    #[serde(default)]
    id: Value,
    #[serde(default)]
    title: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct PaginatedObservations {
    #[serde(default, alias = "items", alias = "data")]
    observations: Vec<ClaudeMemObservation>,
}

#[derive(Debug, Clone, Deserialize)]
struct SearchResponse {
    #[serde(default)]
    observations: Vec<ClaudeMemObservation>,
}

#[derive(Debug, Clone, Deserialize)]
struct ContextResponse {
    #[serde(default)]
    context: Option<String>,
    #[serde(default)]
    count: Option<usize>,
}

#[derive(Debug, Clone, Deserialize)]
struct ClaudeMemObservation {
    id: Value,
    #[serde(default)]
    project: Option<String>,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    subtitle: Option<String>,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    narrative: Option<String>,
    #[serde(default)]
    r#type: Option<String>,
    #[serde(default)]
    score: Option<f32>,
    #[serde(default)]
    rank: Option<f32>,
    #[serde(default)]
    created_at: Option<String>,
}

#[derive(Debug, Serialize)]
struct SaveMemoryRequest<'a> {
    text: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    title: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    project: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    metadata: Option<&'a Value>,
}

#[derive(Debug, Serialize)]
struct SemanticContextRequest<'a> {
    q: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    project: Option<&'a str>,
    limit: usize,
}

impl ClaudeMemClient {
    pub fn new() -> Result<Self, String> {
        let base_url =
            std::env::var("FROGCLAW_CLAUDE_MEM_URL").unwrap_or_else(|_| DEFAULT_BASE_URL.into());
        let client = Client::builder()
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(120))
            .build()
            .map_err(|e| format!("Failed to create claude-mem HTTP client: {e}"))?;
        Ok(Self { client, base_url })
    }

    pub fn namespace() -> MemoryNamespace {
        MemoryNamespace {
            id: DEFAULT_NAMESPACE_ID.to_string(),
            name: DEFAULT_NAMESPACE_NAME.to_string(),
            scope: "global".to_string(),
            embedding_provider: Some("claude-mem".to_string()),
            embedding_dimensions: None,
            retrieval_threshold: None,
            retrieval_top_k: Some(10),
            icon_type: Some("lucide".to_string()),
            icon_value: Some("Brain".to_string()),
            sort_order: 0,
        }
    }

    pub fn project_profile(project_path: &str, project_name: Option<&str>) -> ProjectMemoryProfile {
        ProjectMemoryProfile {
            project_path: normalize_project_path(project_path),
            project_name: project_name
                .filter(|v| !v.trim().is_empty())
                .map(|v| v.trim().to_string())
                .unwrap_or_else(|| fallback_project_name(project_path)),
            namespace_id: DEFAULT_NAMESPACE_ID.to_string(),
            enabled: true,
            embedding_provider: Some("claude-mem".to_string()),
            embedding_dimensions: None,
            retrieval_threshold: None,
            retrieval_top_k: Some(10),
            item_count: 0,
            pending_count: 0,
            failed_count: 0,
        }
    }

    pub async fn ensure_ready(&self) -> Result<(), String> {
        append_memory_log("ensure_ready start");
        cleanup_leftover_worker_if_unmanaged().await;

        if self.health().await.is_ok() {
            append_memory_log("ensure_ready ready existing_worker=true");
            return Ok(());
        }

        let _guard = START_LOCK.get_or_init(|| Mutex::new(())).lock().await;
        if self.health().await.is_ok() {
            append_memory_log("ensure_ready ready existing_worker=true after_lock=true");
            return Ok(());
        }

        append_memory_log("ensure_ready worker_start required");
        start_managed_worker().await.map_err(|err| {
            append_memory_log(format!(
                "ensure_ready worker_start failed error={}",
                compact_log_value(&err, 240)
            ));
            err
        })?;
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(20) {
            if self.health().await.is_ok() {
                append_memory_log(format!(
                    "ensure_ready ready existing_worker=false elapsed_ms={}",
                    start.elapsed().as_millis()
                ));
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(400)).await;
        }
        let err = "claude-mem worker did not become ready on http://127.0.0.1:37777".to_string();
        append_memory_log(format!(
            "ensure_ready failed error={}",
            compact_log_value(&err, 240)
        ));
        Err(err)
    }

    async fn health(&self) -> Result<(), String> {
        let url = format!("{}/api/health", self.base_url);
        let response = self
            .client
            .get(url)
            .send()
            .await
            .map_err(|e| e.to_string())?;
        if response.status().is_success() {
            Ok(())
        } else {
            Err(format!("claude-mem health returned {}", response.status()))
        }
    }

    pub async fn save_memory(&self, input: ClaudeMemSaveInput) -> Result<MemoryItem, String> {
        let started = Instant::now();
        let project = input
            .project
            .as_deref()
            .map(|value| compact_log_value(value, 180))
            .unwrap_or_else(|| "-".to_string());
        append_memory_log(format!(
            "save_memory start title_chars={} project={} text_chars={}",
            input
                .title
                .as_deref()
                .map(|value| value.chars().count())
                .unwrap_or(0),
            project,
            input.text.chars().count()
        ));
        self.ensure_ready().await?;
        let response = match self.send_save_request(&input).await {
            Ok(response) => response,
            Err(first_error) => {
                tracing::warn!(
                    "claude-mem save failed, restarting local worker and retrying once: {}",
                    first_error
                );
                append_memory_log(format!(
                    "save_memory request_failed retrying=true error={}",
                    compact_log_value(&first_error, 240)
                ));
                restart_managed_worker().await?;
                self.ensure_ready().await?;
                self.send_save_request(&input)
                    .await
                    .map_err(|second_error| {
                        let err = format!(
                            "claude-mem save failed after restart: {second_error}; first error: {first_error}"
                        );
                        append_memory_log(format!(
                            "save_memory failed elapsed_ms={} error={}",
                            started.elapsed().as_millis(),
                            compact_log_value(&err, 320)
                        ));
                        err
                    })?
            }
        };
        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            let err = format!("claude-mem save returned {status}: {body}");
            append_memory_log(format!(
                "save_memory failed elapsed_ms={} status={} error={}",
                started.elapsed().as_millis(),
                status,
                compact_log_value(&err, 320)
            ));
            return Err(err);
        }
        let saved: SaveMemoryResponse = response.json().await.map_err(|e| {
            let err = format!("claude-mem save response parse failed: {e}");
            append_memory_log(format!(
                "save_memory failed elapsed_ms={} error={}",
                started.elapsed().as_millis(),
                compact_log_value(&err, 240)
            ));
            err
        })?;
        let saved_id = value_to_id(&saved.id);
        append_memory_log(format!(
            "save_memory ok id={} elapsed_ms={}",
            compact_log_value(&saved_id, 120),
            started.elapsed().as_millis()
        ));
        Ok(MemoryItem {
            id: saved_id,
            namespace_id: DEFAULT_NAMESPACE_ID.to_string(),
            title: saved
                .title
                .or(input.title)
                .unwrap_or_else(|| title_from_text(&input.text)),
            content: input.text,
            source: "manual".to_string(),
            index_status: "ready".to_string(),
            index_error: None,
            updated_at: chrono::Utc::now().to_rfc3339(),
        })
    }

    async fn send_save_request(
        &self,
        input: &ClaudeMemSaveInput,
    ) -> Result<reqwest::Response, String> {
        let request = SaveMemoryRequest {
            text: input.text.trim(),
            title: input.title.as_deref().filter(|v| !v.trim().is_empty()),
            project: input.project.as_deref().filter(|v| !v.trim().is_empty()),
            metadata: input.metadata.as_ref(),
        };
        self.client
            .post(format!("{}/api/memory/save", self.base_url))
            .json(&request)
            .send()
            .await
            .map_err(|e| e.to_string())
    }

    pub async fn list_items(
        &self,
        project: Option<&str>,
        limit: usize,
    ) -> Result<Vec<MemoryItem>, String> {
        let started = Instant::now();
        append_memory_log(format!(
            "list_items start project={} limit={}",
            project
                .map(|value| compact_log_value(value, 180))
                .unwrap_or_else(|| "-".to_string()),
            limit
        ));
        self.ensure_ready().await?;
        let mut request = self
            .client
            .get(format!("{}/api/observations", self.base_url))
            .query(&[("offset", "0"), ("limit", &limit.to_string())]);
        if let Some(project) = project.filter(|v| !v.trim().is_empty()) {
            request = request.query(&[("project", project)]);
        }
        let response = send_with_short_retry(request, "list_items", started)
            .await
            .map_err(|e| {
                let err = format!("claude-mem list failed: {e}");
                append_memory_log(format!(
                    "list_items failed elapsed_ms={} error={}",
                    started.elapsed().as_millis(),
                    compact_log_value(&err, 240)
                ));
                err
            })?;
        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            let err = format!("claude-mem list returned {status}: {body}");
            append_memory_log(format!(
                "list_items failed elapsed_ms={} status={} error={}",
                started.elapsed().as_millis(),
                status,
                compact_log_value(&err, 320)
            ));
            return Err(err);
        }
        let body: Value = response.json().await.map_err(|e| {
            let err = format!("claude-mem list response parse failed: {e}");
            append_memory_log(format!(
                "list_items failed elapsed_ms={} error={}",
                started.elapsed().as_millis(),
                compact_log_value(&err, 240)
            ));
            err
        })?;
        let items = parse_observation_list(body)
            .into_iter()
            .map(observation_to_item)
            .collect::<Vec<_>>();
        append_memory_log(format!(
            "list_items ok count={} elapsed_ms={}",
            items.len(),
            started.elapsed().as_millis()
        ));
        Ok(items)
    }

    pub async fn search_memory(
        &self,
        query: &str,
        project: Option<&str>,
        limit: usize,
    ) -> Result<Vec<frogclaw_core::vector_store::VectorSearchResult>, String> {
        let started = Instant::now();
        append_memory_log(format!(
            "search_memory start query_chars={} project={} limit={}",
            query.chars().count(),
            project
                .map(|value| compact_log_value(value, 180))
                .unwrap_or_else(|| "-".to_string()),
            limit
        ));
        self.ensure_ready().await?;
        let mut request = self
            .client
            .get(format!("{}/api/search", self.base_url))
            .query(&[
                ("query", query),
                ("searchType", "observations"),
                ("format", "json"),
                ("limit", &limit.to_string()),
            ]);
        if let Some(project) = project.filter(|v| !v.trim().is_empty()) {
            request = request.query(&[("project", project)]);
        }
        let response = send_with_short_retry(request, "search_memory", started)
            .await
            .map_err(|e| {
                let err = format!("claude-mem search failed: {e}");
                append_memory_log(format!(
                    "search_memory failed elapsed_ms={} error={}",
                    started.elapsed().as_millis(),
                    compact_log_value(&err, 240)
                ));
                err
            })?;
        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            let err = format!("claude-mem search returned {status}: {body}");
            append_memory_log(format!(
                "search_memory failed elapsed_ms={} status={} error={}",
                started.elapsed().as_millis(),
                status,
                compact_log_value(&err, 320)
            ));
            return Err(err);
        }
        let body: Value = response.json().await.map_err(|e| {
            let err = format!("claude-mem search response parse failed: {e}");
            append_memory_log(format!(
                "search_memory failed elapsed_ms={} error={}",
                started.elapsed().as_millis(),
                compact_log_value(&err, 240)
            ));
            err
        })?;
        let observations = parse_search_observations(body);
        let results = observations
            .into_iter()
            .take(limit)
            .enumerate()
            .map(|(idx, obs)| observation_to_vector_result(obs, idx))
            .collect::<Vec<_>>();
        append_memory_log(format!(
            "search_memory ok count={} elapsed_ms={}",
            results.len(),
            started.elapsed().as_millis()
        ));
        Ok(results)
    }

    pub async fn collect_context(
        &self,
        query: &str,
        project: Option<&str>,
        limit: usize,
    ) -> Result<RagContextResult, String> {
        let started = Instant::now();
        append_memory_log(format!(
            "collect_context start query_chars={} project={} limit={}",
            query.chars().count(),
            project
                .map(|value| compact_log_value(value, 180))
                .unwrap_or_else(|| "-".to_string()),
            limit
        ));
        self.ensure_ready().await?;
        let semantic_response = self
            .client
            .post(format!("{}/api/context/semantic", self.base_url))
            .json(&SemanticContextRequest {
                q: query,
                project: project.filter(|v| !v.trim().is_empty()),
                limit,
            })
            .send()
            .await
            .map_err(|e| {
                let err = format!("claude-mem semantic context failed: {e}");
                append_memory_log(format!(
                    "collect_context semantic_failed elapsed_ms={} error={}",
                    started.elapsed().as_millis(),
                    compact_log_value(&err, 240)
                ));
                err
            })?;
        if semantic_response.status().is_success() {
            let body: ContextResponse = semantic_response.json().await.map_err(|e| {
                let err = format!("claude-mem semantic context response parse failed: {e}");
                append_memory_log(format!(
                    "collect_context semantic_failed elapsed_ms={} error={}",
                    started.elapsed().as_millis(),
                    compact_log_value(&err, 240)
                ));
                err
            })?;
            let context = body.context.unwrap_or_default();
            if !context.trim().is_empty() {
                let count = body.count.unwrap_or(1).max(1);
                append_memory_log(format!(
                    "collect_context ok source=semantic count={} elapsed_ms={}",
                    count,
                    started.elapsed().as_millis()
                ));
                return Ok(context_to_rag_result(context, count));
            }
            append_memory_log(format!(
                "collect_context semantic_empty elapsed_ms={}",
                started.elapsed().as_millis()
            ));
        } else {
            tracing::warn!(
                "claude-mem semantic context returned {}. Falling back to inject/search.",
                semantic_response.status()
            );
            append_memory_log(format!(
                "collect_context semantic_status status={} fallback=inject",
                semantic_response.status()
            ));
        }

        let search_results = match self.search_memory(query, project, limit).await {
            Ok(results) => results,
            Err(err) => {
                append_memory_log(format!(
                    "collect_context search_failed elapsed_ms={} error={}",
                    started.elapsed().as_millis(),
                    compact_log_value(&err, 240)
                ));
                Vec::new()
            }
        };
        let search_count = search_results.len();
        let useful_search_results = filter_meaningful_memory_results(search_results);
        if search_count == 0 {
            append_memory_log(format!(
                "collect_context search_empty elapsed_ms={}",
                started.elapsed().as_millis()
            ));
        } else if useful_search_results.len() < search_count {
            append_memory_log(format!(
                "collect_context search_low_value_filtered count={} kept={} elapsed_ms={}",
                search_count - useful_search_results.len(),
                useful_search_results.len(),
                started.elapsed().as_millis()
            ));
        }

        let prefer_recent = prefers_recent_context(query);
        let should_fetch_recent = useful_search_results.is_empty() || prefer_recent;
        let recent_results = if should_fetch_recent {
            match self.list_items(project, limit.max(8)).await {
                Ok(recent_items) => recent_memory_results(query, recent_items, limit),
                Err(err) => {
                    append_memory_log(format!(
                        "collect_context recent_failed elapsed_ms={} error={}",
                        started.elapsed().as_millis(),
                        compact_log_value(&err, 240)
                    ));
                    Vec::new()
                }
            }
        } else {
            Vec::new()
        };
        if recent_results.is_empty() {
            append_memory_log(format!(
                "collect_context recent_empty elapsed_ms={}",
                started.elapsed().as_millis()
            ));
        }

        let combined_results = if prefer_recent {
            merge_memory_results(recent_results, useful_search_results, limit.max(1))
        } else {
            merge_memory_results(useful_search_results, recent_results, limit.max(1))
        };
        if !combined_results.is_empty() {
            let used_search = combined_results.iter().any(|result| result.has_embedding);
            let used_recent = combined_results
                .iter()
                .any(|result| result.id.starts_with("claude-mem-recent-"));
            let source = match (used_search, used_recent) {
                (true, true) => "search_recent",
                (true, false) => "search",
                (false, true) => "recent_observations",
                (false, false) => "unknown",
            };
            append_memory_log(format!(
                "collect_context ok source={} count={} elapsed_ms={}",
                source,
                combined_results.len(),
                started.elapsed().as_millis()
            ));
            return Ok(vector_results_to_rag_result(combined_results));
        }

        let mut request = self
            .client
            .get(format!("{}/api/context/inject", self.base_url))
            .query(&[("limit", limit.to_string()), ("full", "true".to_string())]);
        if let Some(project) = project.filter(|v| !v.trim().is_empty()) {
            request = request.query(&[("projects", project)]);
        }
        let response = request.send().await.map_err(|e| {
            let err = format!("claude-mem context failed: {e}");
            append_memory_log(format!(
                "collect_context inject_failed elapsed_ms={} error={}",
                started.elapsed().as_millis(),
                compact_log_value(&err, 240)
            ));
            err
        })?;
        if response.status().is_success() {
            let context = response.text().await.map_err(|e| {
                let err = format!("claude-mem context response read failed: {e}");
                append_memory_log(format!(
                    "collect_context inject_failed elapsed_ms={} error={}",
                    started.elapsed().as_millis(),
                    compact_log_value(&err, 240)
                ));
                err
            })?;
            if is_useful_injected_context(&context) {
                append_memory_log(format!(
                    "collect_context ok source=inject count=1 elapsed_ms={}",
                    started.elapsed().as_millis()
                ));
                return Ok(context_to_rag_result(context, 1));
            }
            append_memory_log(format!(
                "collect_context inject_empty elapsed_ms={}",
                started.elapsed().as_millis()
            ));
        } else {
            tracing::warn!("claude-mem context returned {}.", response.status());
            append_memory_log(format!(
                "collect_context inject_status status={}",
                response.status()
            ));
        }

        append_memory_log(format!(
            "collect_context ok source=none count=0 elapsed_ms={}",
            started.elapsed().as_millis()
        ));
        Ok(RagContextResult {
            context_parts: vec![],
            source_results: vec![],
        })
    }
}

async fn send_with_short_retry(
    request: reqwest::RequestBuilder,
    event: &str,
    started: Instant,
) -> Result<reqwest::Response, String> {
    let retry_request = request.try_clone();
    match request.send().await {
        Ok(response) => Ok(response),
        Err(first_error) => {
            let first = reqwest_error_detail(&first_error);
            append_memory_log(format!(
                "{} request_failed retrying=true elapsed_ms={} error={}",
                event,
                started.elapsed().as_millis(),
                compact_log_value(&first, 320)
            ));
            let Some(retry_request) = retry_request else {
                return Err(first);
            };
            tokio::time::sleep(Duration::from_millis(200)).await;
            retry_request.send().await.map_err(|second_error| {
                format!(
                    "{}; first_error={}",
                    reqwest_error_detail(&second_error),
                    compact_log_value(&first, 220)
                )
            })
        }
    }
}

fn reqwest_error_detail(error: &reqwest::Error) -> String {
    let mut parts = vec![error.to_string()];
    if let Some(status) = error.status() {
        parts.push(format!("status={status}"));
    }
    if let Some(url) = error.url() {
        parts.push(format!("url={url}"));
    }
    let mut source = std::error::Error::source(error);
    while let Some(err) = source {
        parts.push(format!("source={err}"));
        source = std::error::Error::source(err);
    }
    parts.join(" ")
}

pub fn init_resource_dir(path: PathBuf) {
    let _ = RESOURCE_DIR.set(path);
}

async fn build_runtime_config(
    db: &DatabaseConnection,
    master_key: &[u8; 32],
) -> Result<ClaudeMemRuntimeConfig, String> {
    let settings = frogclaw_core::repo::settings::get_settings(db)
        .await
        .unwrap_or_default();
    let providers = frogclaw_core::repo::provider::list_providers(db)
        .await
        .map_err(|e| format!("读取模型服务商失败: {e}"))?;

    if let Some((provider, api_key)) = choose_provider_with_key(
        db,
        master_key,
        &providers,
        &settings,
        ProviderType::Anthropic,
    )
    .await?
    {
        let model_id = choose_anthropic_model(&provider, &settings);
        let base_url = anthropic_gateway_base_url(&provider);
        return Ok(make_claude_gateway_config(
            provider.id,
            model_id,
            base_url,
            api_key,
        ));
    }

    if let Some((provider, api_key)) =
        choose_provider_with_key(db, master_key, &providers, &settings, ProviderType::Gemini)
            .await?
    {
        let model_id = choose_gemini_model(&provider, &settings);
        return Ok(make_gemini_config(provider.id, model_id, api_key));
    }

    if let Some((provider, api_key)) =
        choose_openrouter_provider(db, master_key, &providers, &settings).await?
    {
        let model_id = choose_openrouter_model(&provider, &settings);
        return Ok(make_openrouter_config(provider.id, model_id, api_key));
    }

    append_memory_log("runtime_config no_compatible_frogclaw_provider fallback=default_claude");
    Ok(make_default_claude_config())
}

async fn choose_provider_with_key(
    db: &DatabaseConnection,
    master_key: &[u8; 32],
    providers: &[ProviderConfig],
    settings: &AppSettings,
    provider_type: ProviderType,
) -> Result<Option<(ProviderConfig, String)>, String> {
    let mut candidates = compatible_providers(providers, &provider_type);
    sort_by_settings_preference(&mut candidates, settings);
    first_provider_with_key(db, master_key, candidates).await
}

async fn choose_openrouter_provider(
    db: &DatabaseConnection,
    master_key: &[u8; 32],
    providers: &[ProviderConfig],
    settings: &AppSettings,
) -> Result<Option<(ProviderConfig, String)>, String> {
    let mut candidates = providers
        .iter()
        .filter(|provider| {
            provider.enabled
                && matches!(
                    provider.provider_type,
                    ProviderType::OpenAI | ProviderType::OpenAIResponses | ProviderType::Custom
                )
                && looks_like_openrouter_provider(provider)
        })
        .collect::<Vec<_>>();
    sort_by_settings_preference(&mut candidates, settings);
    first_provider_with_key(db, master_key, candidates).await
}

async fn first_provider_with_key(
    db: &DatabaseConnection,
    master_key: &[u8; 32],
    candidates: Vec<&ProviderConfig>,
) -> Result<Option<(ProviderConfig, String)>, String> {
    for provider in candidates {
        let key_row = match frogclaw_core::repo::provider::get_active_key(db, &provider.id).await {
            Ok(key) => key,
            Err(err) => {
                append_memory_log(format!(
                    "runtime_config skip_provider provider_id={} reason=no_active_key error={}",
                    provider.id,
                    compact_log_value(&err.to_string(), 180)
                ));
                continue;
            }
        };
        let api_key = frogclaw_core::crypto::decrypt_key(&key_row.key_encrypted, master_key)
            .map_err(|e| format!("解密服务商密钥失败 provider_id={} error={e}", provider.id))?;
        if api_key.trim().is_empty() {
            append_memory_log(format!(
                "runtime_config skip_provider provider_id={} reason=empty_key",
                provider.id
            ));
            continue;
        }
        return Ok(Some((provider.clone(), api_key)));
    }
    Ok(None)
}

fn compatible_providers<'a>(
    providers: &'a [ProviderConfig],
    provider_type: &ProviderType,
) -> Vec<&'a ProviderConfig> {
    providers
        .iter()
        .filter(|provider| provider.enabled && &provider.provider_type == provider_type)
        .collect()
}

fn sort_by_settings_preference(providers: &mut Vec<&ProviderConfig>, settings: &AppSettings) {
    let preferred = [
        settings.title_summary_provider_id.as_deref(),
        settings.default_provider_id.as_deref(),
    ];
    providers.sort_by_key(|provider| {
        preferred
            .iter()
            .position(|id| *id == Some(provider.id.as_str()))
            .unwrap_or(preferred.len())
    });
}

fn choose_anthropic_model(provider: &ProviderConfig, settings: &AppSettings) -> String {
    choose_model_by_preferences(
        provider,
        settings,
        DEFAULT_CLAUDE_MEM_MODEL,
        &["claude-haiku-4-5", "claude-3-5-haiku", "haiku", "claude"],
    )
}

fn choose_gemini_model(provider: &ProviderConfig, settings: &AppSettings) -> String {
    let candidate = choose_model_by_preferences(
        provider,
        settings,
        DEFAULT_GEMINI_MEM_MODEL,
        &[
            "gemini-2.5-flash-lite",
            "gemini-2.5-flash",
            "gemini-2.0-flash",
        ],
    );
    if is_claude_mem_supported_gemini_model(&candidate) {
        candidate
    } else {
        DEFAULT_GEMINI_MEM_MODEL.to_string()
    }
}

fn choose_openrouter_model(provider: &ProviderConfig, settings: &AppSettings) -> String {
    choose_model_by_preferences(
        provider,
        settings,
        "xiaomi/mimo-v2-flash:free",
        &["claude", "gemini", "gpt-4o-mini", "flash", "free"],
    )
}

fn choose_model_by_preferences(
    provider: &ProviderConfig,
    settings: &AppSettings,
    fallback: &str,
    preferred_needles: &[&str],
) -> String {
    if settings.title_summary_provider_id.as_deref() == Some(provider.id.as_str()) {
        if let Some(model_id) = settings
            .title_summary_model_id
            .as_deref()
            .filter(|value| !value.trim().is_empty())
        {
            return model_id.trim().to_string();
        }
    }
    if settings.default_provider_id.as_deref() == Some(provider.id.as_str()) {
        if let Some(model_id) = settings
            .default_model_id
            .as_deref()
            .filter(|value| !value.trim().is_empty())
        {
            return model_id.trim().to_string();
        }
    }

    let enabled_chat = provider
        .models
        .iter()
        .filter(|model| model.enabled && model.model_type == ModelType::Chat)
        .collect::<Vec<_>>();
    for needle in preferred_needles {
        if let Some(model) = enabled_chat
            .iter()
            .find(|model| model.model_id.to_lowercase().contains(needle))
        {
            return model.model_id.clone();
        }
    }
    enabled_chat
        .first()
        .map(|model| model.model_id.clone())
        .unwrap_or_else(|| fallback.to_string())
}

fn is_claude_mem_supported_gemini_model(model_id: &str) -> bool {
    matches!(
        model_id,
        "gemini-2.5-flash-lite"
            | "gemini-2.5-flash"
            | "gemini-2.5-pro"
            | "gemini-2.0-flash"
            | "gemini-2.0-flash-lite"
            | "gemini-3-flash"
            | "gemini-3-flash-preview"
    )
}

fn looks_like_openrouter_provider(provider: &ProviderConfig) -> bool {
    let haystack = format!(
        "{} {} {} {}",
        provider.id,
        provider.name,
        provider.api_host,
        provider.builtin_id.as_deref().unwrap_or_default()
    )
    .to_lowercase();
    haystack.contains("openrouter")
}

fn anthropic_gateway_base_url(provider: &ProviderConfig) -> String {
    frogclaw_providers::resolve_base_url_for_type(&provider.api_host, &provider.provider_type)
}

fn make_base_settings() -> BTreeMap<String, String> {
    BTreeMap::from([
        (
            "CLAUDE_MEM_DATA_DIR".to_string(),
            claude_mem_data_dir().to_string_lossy().to_string(),
        ),
        (
            "CLAUDE_MEM_WORKER_HOST".to_string(),
            CLAUDE_MEM_WORKER_HOST.to_string(),
        ),
        (
            "CLAUDE_MEM_WORKER_PORT".to_string(),
            CLAUDE_MEM_WORKER_PORT.to_string(),
        ),
    ])
}

fn make_claude_gateway_config(
    provider_id: String,
    model_id: String,
    base_url: String,
    api_key: String,
) -> ClaudeMemRuntimeConfig {
    let mut settings = make_base_settings();
    settings.insert("CLAUDE_MEM_PROVIDER".to_string(), "claude".to_string());
    settings.insert(
        "CLAUDE_MEM_CLAUDE_AUTH_METHOD".to_string(),
        "gateway".to_string(),
    );
    settings.insert("CLAUDE_MEM_MODEL".to_string(), model_id.clone());
    settings.insert("CLAUDE_MEM_GEMINI_API_KEY".to_string(), String::new());
    settings.insert("CLAUDE_MEM_OPENROUTER_API_KEY".to_string(), String::new());

    let mut env_file = BTreeMap::new();
    env_file.insert("ANTHROPIC_API_KEY".to_string(), String::new());
    env_file.insert("ANTHROPIC_BASE_URL".to_string(), base_url.clone());
    env_file.insert("ANTHROPIC_AUTH_TOKEN".to_string(), api_key.clone());
    env_file.insert("GEMINI_API_KEY".to_string(), String::new());
    env_file.insert("OPENROUTER_API_KEY".to_string(), String::new());

    let mut process_env = settings.clone();
    process_env.insert(
        "CLAUDE_MEM_ENV_FILE".to_string(),
        claude_mem_env_file().to_string_lossy().to_string(),
    );
    process_env.insert("ANTHROPIC_BASE_URL".to_string(), base_url.clone());
    process_env.insert("ANTHROPIC_AUTH_TOKEN".to_string(), api_key);

    ClaudeMemRuntimeConfig {
        provider: ClaudeMemProviderConfig::ClaudeGateway {
            provider_id,
            model_id,
        },
        settings,
        env_file,
        process_env,
    }
}

fn make_gemini_config(
    provider_id: String,
    model_id: String,
    api_key: String,
) -> ClaudeMemRuntimeConfig {
    let mut settings = make_base_settings();
    settings.insert("CLAUDE_MEM_PROVIDER".to_string(), "gemini".to_string());
    settings.insert("CLAUDE_MEM_GEMINI_MODEL".to_string(), model_id.clone());
    settings.insert("CLAUDE_MEM_GEMINI_API_KEY".to_string(), api_key.clone());
    settings.insert("CLAUDE_MEM_OPENROUTER_API_KEY".to_string(), String::new());

    let mut env_file = BTreeMap::new();
    env_file.insert("ANTHROPIC_API_KEY".to_string(), String::new());
    env_file.insert("ANTHROPIC_BASE_URL".to_string(), String::new());
    env_file.insert("ANTHROPIC_AUTH_TOKEN".to_string(), String::new());
    env_file.insert("GEMINI_API_KEY".to_string(), api_key.clone());
    env_file.insert("OPENROUTER_API_KEY".to_string(), String::new());

    let mut process_env = settings.clone();
    process_env.insert(
        "CLAUDE_MEM_ENV_FILE".to_string(),
        claude_mem_env_file().to_string_lossy().to_string(),
    );
    process_env.insert("GEMINI_API_KEY".to_string(), api_key);

    ClaudeMemRuntimeConfig {
        provider: ClaudeMemProviderConfig::Gemini {
            provider_id,
            model_id,
        },
        settings,
        env_file,
        process_env,
    }
}

fn make_openrouter_config(
    provider_id: String,
    model_id: String,
    api_key: String,
) -> ClaudeMemRuntimeConfig {
    let mut settings = make_base_settings();
    settings.insert("CLAUDE_MEM_PROVIDER".to_string(), "openrouter".to_string());
    settings.insert("CLAUDE_MEM_OPENROUTER_MODEL".to_string(), model_id.clone());
    settings.insert("CLAUDE_MEM_OPENROUTER_API_KEY".to_string(), api_key.clone());
    settings.insert("CLAUDE_MEM_GEMINI_API_KEY".to_string(), String::new());

    let mut env_file = BTreeMap::new();
    env_file.insert("ANTHROPIC_API_KEY".to_string(), String::new());
    env_file.insert("ANTHROPIC_BASE_URL".to_string(), String::new());
    env_file.insert("ANTHROPIC_AUTH_TOKEN".to_string(), String::new());
    env_file.insert("GEMINI_API_KEY".to_string(), String::new());
    env_file.insert("OPENROUTER_API_KEY".to_string(), api_key.clone());

    let mut process_env = settings.clone();
    process_env.insert(
        "CLAUDE_MEM_ENV_FILE".to_string(),
        claude_mem_env_file().to_string_lossy().to_string(),
    );
    process_env.insert("OPENROUTER_API_KEY".to_string(), api_key);

    ClaudeMemRuntimeConfig {
        provider: ClaudeMemProviderConfig::OpenRouter {
            provider_id,
            model_id,
        },
        settings,
        env_file,
        process_env,
    }
}

fn make_default_claude_config() -> ClaudeMemRuntimeConfig {
    let mut settings = make_base_settings();
    settings.insert("CLAUDE_MEM_PROVIDER".to_string(), "claude".to_string());
    settings.insert(
        "CLAUDE_MEM_CLAUDE_AUTH_METHOD".to_string(),
        "subscription".to_string(),
    );
    settings.insert(
        "CLAUDE_MEM_MODEL".to_string(),
        DEFAULT_CLAUDE_MEM_MODEL.to_string(),
    );
    settings.insert("CLAUDE_MEM_GEMINI_API_KEY".to_string(), String::new());
    settings.insert("CLAUDE_MEM_OPENROUTER_API_KEY".to_string(), String::new());

    let mut env_file = BTreeMap::new();
    env_file.insert("ANTHROPIC_API_KEY".to_string(), String::new());
    env_file.insert("ANTHROPIC_BASE_URL".to_string(), String::new());
    env_file.insert("ANTHROPIC_AUTH_TOKEN".to_string(), String::new());
    env_file.insert("GEMINI_API_KEY".to_string(), String::new());
    env_file.insert("OPENROUTER_API_KEY".to_string(), String::new());

    let mut process_env = settings.clone();
    process_env.insert(
        "CLAUDE_MEM_ENV_FILE".to_string(),
        claude_mem_env_file().to_string_lossy().to_string(),
    );

    ClaudeMemRuntimeConfig {
        provider: ClaudeMemProviderConfig::DefaultClaude,
        settings,
        env_file,
        process_env,
    }
}

async fn refresh_runtime_config(db: &DatabaseConnection, master_key: &[u8; 32]) {
    match build_runtime_config(db, master_key).await {
        Ok(config) => {
            append_memory_log(format!(
                "runtime_config ok provider={} provider_id={} model={} auth=from_frogclaw",
                config.provider.label(),
                config.provider.provider_id().unwrap_or("-"),
                config.provider.model_id().unwrap_or("-")
            ));
            let mut guard = RUNTIME_CONFIG.get_or_init(|| Mutex::new(None)).lock().await;
            *guard = Some(config);
        }
        Err(err) => {
            append_memory_log(format!(
                "runtime_config failed error={}",
                compact_log_value(&err, 240)
            ));
        }
    }
}

pub fn start_background_worker(db: DatabaseConnection, master_key: [u8; 32]) {
    append_memory_log("background_worker start requested");
    tauri::async_runtime::spawn(async move {
        refresh_runtime_config(&db, &master_key).await;
        match ClaudeMemClient::new() {
            Ok(client) => {
                if let Err(err) = client.ensure_ready().await {
                    tracing::warn!("claude-mem auto-start failed: {}", err);
                    append_memory_log(format!(
                        "background_worker auto_start failed error={}",
                        compact_log_value(&err, 240)
                    ));
                } else {
                    tracing::info!("claude-mem worker is ready");
                    append_memory_log("background_worker auto_start ready");
                }
            }
            Err(err) => {
                tracing::warn!("claude-mem client init failed: {}", err);
                append_memory_log(format!(
                    "background_worker client_init failed error={}",
                    compact_log_value(&err, 240)
                ));
            }
        }
    });
    tauri::async_runtime::spawn(async {
        monitor_worker().await;
    });
}

pub fn shutdown_managed_worker() {
    SHUTTING_DOWN.store(true, std::sync::atomic::Ordering::SeqCst);
    append_memory_log("shutdown managed_worker requested");
    if let Some(process) = WORKER_PROCESS.get() {
        if let Ok(mut guard) = process.try_lock() {
            if let Some(mut child) = guard.take() {
                let pid = child.id();
                tracing::info!("Stopping managed claude-mem worker pid {}", pid);
                append_memory_log(format!("shutdown stopping_worker pid={}", pid));
                kill_process_tree(pid, &mut child);
                let _ = child.wait();
                append_memory_log(format!("shutdown stopped_worker pid={}", pid));
            }
        }
    }
}

async fn monitor_worker() {
    let mut interval = tokio::time::interval(Duration::from_secs(5));
    loop {
        interval.tick().await;
        if SHUTTING_DOWN.load(std::sync::atomic::Ordering::SeqCst) {
            return;
        }
        let client = match ClaudeMemClient::new() {
            Ok(client) => client,
            Err(err) => {
                tracing::warn!("claude-mem monitor client init failed: {}", err);
                append_memory_log(format!(
                    "monitor client_init failed error={}",
                    compact_log_value(&err, 240)
                ));
                continue;
            }
        };
        if client.health().await.is_ok() {
            reap_finished_worker().await;
            continue;
        }
        let _guard = START_LOCK.get_or_init(|| Mutex::new(())).lock().await;
        if client.health().await.is_ok() {
            continue;
        }
        append_memory_log("monitor worker_unhealthy restarting=true");
        if let Err(err) = restart_managed_worker().await {
            tracing::warn!("claude-mem monitor restart failed: {}", err);
            append_memory_log(format!(
                "monitor restart failed error={}",
                compact_log_value(&err, 240)
            ));
        } else {
            append_memory_log("monitor restart ok");
        }
    }
}

pub async fn save_auto_memory(
    project_path: Option<&str>,
    project_name: Option<&str>,
    title: &str,
    text: &str,
    source: &str,
) -> Result<MemoryItem, String> {
    append_memory_log(format!(
        "save_auto_memory start source={} project_path={} project_name={} title_chars={}",
        compact_log_value(source, 80),
        project_path
            .map(|value| compact_log_value(value, 180))
            .unwrap_or_else(|| "-".to_string()),
        project_name
            .map(|value| compact_log_value(value, 120))
            .unwrap_or_else(|| "-".to_string()),
        title.chars().count()
    ));
    let client = ClaudeMemClient::new()?;
    let project = project_name
        .filter(|v| !v.trim().is_empty())
        .map(|v| v.trim().to_string())
        .or_else(|| project_path.map(fallback_project_name));
    let metadata = serde_json::json!({
        "projectPath": project_path,
        "projectName": project_name,
        "source": source,
        "app": "frogclaw",
    });
    client
        .save_memory(ClaudeMemSaveInput {
            title: Some(title.to_string()),
            text: text.to_string(),
            project,
            metadata: Some(metadata),
        })
        .await
}

pub async fn collect_project_context(
    query: &str,
    project_path: Option<&str>,
    project_name: Option<&str>,
    limit: usize,
) -> Result<RagContextResult, String> {
    append_memory_log(format!(
        "collect_project_context start query_chars={} project_path={} project_name={} limit={}",
        query.chars().count(),
        project_path
            .map(|value| compact_log_value(value, 180))
            .unwrap_or_else(|| "-".to_string()),
        project_name
            .map(|value| compact_log_value(value, 120))
            .unwrap_or_else(|| "-".to_string()),
        limit
    ));
    let client = ClaudeMemClient::new()?;
    let project = project_name
        .filter(|v| !v.trim().is_empty())
        .or_else(|| project_path.and_then(|p| Path::new(p).file_name().and_then(|v| v.to_str())));
    client.collect_context(query, project, limit).await
}

pub fn memory_log_path() -> PathBuf {
    crate::paths::frogclaw_home().join("memory.log")
}

fn localize_memory_log(message: &str) -> String {
    let replacements = [
        ("conversation_id=", "会话="),
        ("namespace_id=", "命名空间="),
        ("item_id=", "记忆项="),
        ("project_path_chars=", "项目路径字符数="),
        ("project_path=", "项目路径="),
        ("project_name=", "项目="),
        ("project=", "项目="),
        ("query_chars=", "查询字符数="),
        ("title_chars=", "标题字符数="),
        ("content_chars=", "内容字符数="),
        ("text_chars=", "文本字符数="),
        ("user_chars=", "用户字符数="),
        ("assistant_chars=", "助手字符数="),
        ("enabled_memory_count=", "启用记忆数="),
        ("context_parts=", "上下文段数="),
        ("sources=", "来源数="),
        ("source=", "来源="),
        ("count=", "数量="),
        ("limit=", "限制="),
        ("top_k=", "top_k="),
        ("elapsed_ms=", "耗时毫秒="),
        ("saved=", "已保存="),
        ("scanned=", "已扫描="),
        ("candidates=", "候选数="),
        ("provider_id=", "服务商="),
        ("provider=", "服务商类型="),
        ("model=", "模型="),
        ("auth=", "认证="),
        ("settings_path=", "设置文件="),
        ("env_file=", "环境文件="),
        ("registry_key=", "适配器="),
        ("status=", "状态="),
        ("fallback=", "回退="),
        ("retrying=", "重试="),
        ("existing_worker=", "已有记忆进程="),
        ("after_lock=", "加锁后="),
        ("pid=", "进程="),
        ("program=", "程序="),
        ("cwd=", "工作目录="),
        ("args=", "参数="),
        ("path=", "路径="),
        ("error=", "错误="),
        ("reason=", "原因="),
        ("no_active_key", "没有启用密钥"),
        ("empty_key", "密钥为空"),
        ("has_content=", "有内容="),
        ("id=", "ID="),
        ("name_chars=", "名称字符数="),
        ("scope=", "范围="),
    ];

    let mut protected_urls = Vec::new();
    let mut output = protect_log_urls(message, &mut protected_urls);
    for (from, to) in replacements {
        output = output.replace(from, to);
    }

    let event_labels = [
        ("ensure_ready", "检查记忆进程"),
        ("save_auto_memory", "自动保存记忆"),
        ("save_memory", "保存记忆"),
        ("list_items", "列出记忆"),
        ("search_memory", "搜索记忆"),
        ("collect_project_context", "项目记忆上下文"),
        ("collect_context", "收集记忆上下文"),
        ("background_worker", "后台记忆进程"),
        ("runtime_config", "claude-mem 运行配置"),
        ("shutdown", "关闭记忆进程"),
        ("monitor", "监控记忆进程"),
        ("worker", "记忆进程"),
        ("auto_capture", "自动捕获记忆"),
        ("chat_memory_retrieval", "聊天记忆召回"),
        ("command", "记忆命令"),
    ];
    for (from, to) in event_labels {
        output = output.replace(from, to);
    }

    let status_labels = [
        (" start_managed", "：启动托管进程"),
        (" worker_unhealthy", "：进程异常"),
        (" start", "：开始"),
        (" ready", "：就绪"),
        (" requested", "：已请求"),
        (" required", "：需要"),
        (" ok", "：完成"),
        (" failed", "：失败"),
        (" done", "：完成"),
        (" skipped", "：已跳过"),
        (" skip", "：跳过"),
        (" extracted", "：已抽取"),
        (" save_failed", "：保存失败"),
        (" extractor_failed", "：抽取失败"),
        (" extractor_runtime", "：抽取运行时"),
        (" semantic_failed", "：语义上下文失败"),
        (" semantic_empty", "：语义上下文为空"),
        (" semantic_status", "：语义上下文状态异常"),
        (" search_failed", "：搜索失败"),
        (" search_empty", "：搜索为空"),
        (" search_low_value_filtered", "：已过滤低价值搜索结果"),
        (" recent_failed", "：最近项目记忆失败"),
        (" recent_empty", "：最近项目记忆为空"),
        (" inject_failed", "：注入上下文失败"),
        (" inject_empty", "：注入上下文为空"),
        (" inject_status", "：注入上下文状态异常"),
        (" request_failed", "：请求失败"),
        (" client_init", "：客户端初始化"),
        (" auto_start", "：自动启动"),
        (" config_ready", "：配置就绪"),
        (" files_written", "：已写入配置文件"),
        (" skip_provider", "：跳过服务商"),
        (
            " no_compatible_frogclaw_provider",
            "：没有兼容的 FrogClaw 服务商",
        ),
        (" spawn", "：启动进程"),
        (" restart", "：重启"),
        (" stop", "：停止"),
        (" cleanup_leftover", "：清理残留进程"),
        (" exited", "：进程退出"),
        (" poll", "：检查进程"),
        (" killing", "：正在终止"),
    ];
    for (from, to) in status_labels {
        output = output.replace(from, to);
    }

    let command_labels = [
        ("list_memory_namespaces", "列出记忆命名空间"),
        ("list_project_memory_profiles", "列出项目记忆配置"),
        ("get_project_memory_profile", "读取项目记忆配置"),
        ("update_project_memory_profile", "更新项目记忆配置"),
        ("list_project_memory_items", "列出项目记忆项"),
        ("add_project_memory_item", "添加项目记忆"),
        ("summarize_project_memory", "从会话提取项目记忆"),
        ("create_memory_namespace", "创建记忆命名空间"),
        ("delete_memory_namespace", "删除记忆命名空间"),
        ("update_memory_namespace", "更新记忆命名空间"),
        ("list_memory_items", "列出记忆项"),
        ("add_memory_item", "添加记忆"),
        ("delete_memory_item", "删除记忆"),
        ("update_memory_item", "更新记忆"),
        ("search_project_memory", "搜索项目记忆"),
        ("search_memory", "搜索记忆"),
        ("rebuild_memory_index", "重建记忆索引"),
        ("clear_memory_index", "清空记忆索引"),
        ("reindex_memory_item", "重建单条记忆索引"),
        ("reorder_memory_namespaces", "重排记忆命名空间"),
    ];
    for (from, to) in command_labels {
        output = output.replace(from, to);
    }

    let reason_labels = [
        ("worker_start", "启动记忆进程"),
        ("no_child", "没有子进程"),
        ("no_working_directory", "没有项目目录"),
        ("content_too_short", "内容太短"),
        ("no_summary_runtime", "没有可用的记忆抽取模型"),
        ("unsupported_provider", "不支持的模型服务商"),
        ("no_conversations", "当前项目没有会话"),
        (
            "claude_mem_single_local_namespace",
            "claude-mem 只提供单一本地记忆库",
        ),
        (
            "claude_mem_delete_api_unavailable",
            "本地 worker 未提供删除 API",
        ),
        ("claude_mem_managed_index", "索引由 claude-mem 管理"),
    ];
    for (from, to) in reason_labels {
        output = output.replace(from, to);
    }

    let value_labels = [
        ("来源=semantic", "来源=语义上下文"),
        ("来源=search_recent", "来源=搜索+最近项目记忆"),
        ("来源=recent_observations", "来源=最近项目记忆"),
        ("来源=search", "来源=搜索"),
        ("来源=inject", "来源=注入上下文"),
        ("来源=none", "来源=无"),
        ("回退=inject", "回退=注入上下文"),
        ("回退=default_claude", "回退=默认 Claude"),
        ("认证=from_frogclaw", "认证=FrogClaw 用户密钥"),
        ("服务商类型=claude_gateway", "服务商类型=Claude 网关"),
        ("服务商类型=gemini", "服务商类型=Gemini"),
        ("服务商类型=openrouter", "服务商类型=OpenRouter"),
        ("服务商类型=default_claude", "服务商类型=默认 Claude"),
    ];
    for (from, to) in value_labels {
        output = output.replace(from, to);
    }

    restore_log_urls(&output, &protected_urls)
}

fn protect_log_urls(input: &str, urls: &mut Vec<String>) -> String {
    let mut output = String::with_capacity(input.len());
    let mut rest = input;
    loop {
        let Some(pos) = rest.find("http://").or_else(|| rest.find("https://")) else {
            output.push_str(rest);
            break;
        };
        output.push_str(&rest[..pos]);
        let end = rest[pos..]
            .find(|ch: char| ch.is_whitespace() || ch == ')')
            .map(|idx| pos + idx)
            .unwrap_or(rest.len());
        let token = format!("__FROGCLAW_LOG_URL_{}__", urls.len());
        urls.push(rest[pos..end].to_string());
        output.push_str(&token);
        rest = &rest[end..];
    }
    output
}

fn restore_log_urls(input: &str, urls: &[String]) -> String {
    let mut output = input.to_string();
    for (idx, url) in urls.iter().enumerate() {
        output = output.replace(&format!("__FROGCLAW_LOG_URL_{}__", idx), url);
    }
    output
}

pub fn append_memory_log(message: impl AsRef<str>) {
    let path = memory_log_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    const MAX_LOG_BYTES: u64 = 1024 * 1024;
    if std::fs::metadata(&path)
        .map(|metadata| metadata.len() > MAX_LOG_BYTES)
        .unwrap_or(false)
    {
        let rotated = path.with_extension("log.1");
        let _ = std::fs::remove_file(&rotated);
        let _ = std::fs::rename(&path, rotated);
    }
    let timestamp = chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f");
    let line = compact_log_value(&localize_memory_log(message.as_ref()), 900);
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = writeln!(file, "[{timestamp}] {line}");
    }
}

fn parse_observation_list(body: Value) -> Vec<ClaudeMemObservation> {
    if body.is_array() {
        return serde_json::from_value(body).unwrap_or_default();
    }
    if let Ok(parsed) = serde_json::from_value::<PaginatedObservations>(body.clone()) {
        if !parsed.observations.is_empty() {
            return parsed.observations;
        }
    }
    body.get("observations")
        .cloned()
        .or_else(|| body.get("items").cloned())
        .or_else(|| body.get("data").cloned())
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default()
}

fn parse_search_observations(body: Value) -> Vec<ClaudeMemObservation> {
    if body.is_array() {
        return serde_json::from_value(body).unwrap_or_default();
    }
    if let Ok(parsed) = serde_json::from_value::<SearchResponse>(body.clone()) {
        return parsed.observations;
    }
    body.get("observations")
        .cloned()
        .or_else(|| {
            body.get("results")
                .and_then(|v| v.get("observations"))
                .cloned()
        })
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default()
}

fn observation_to_item(obs: ClaudeMemObservation) -> MemoryItem {
    let content = obs_content(&obs);
    MemoryItem {
        id: value_to_id(&obs.id),
        namespace_id: DEFAULT_NAMESPACE_ID.to_string(),
        title: obs
            .title
            .clone()
            .unwrap_or_else(|| title_from_text(&content)),
        content,
        source: "manual".to_string(),
        index_status: "ready".to_string(),
        index_error: None,
        updated_at: obs
            .created_at
            .clone()
            .unwrap_or_else(|| chrono::Utc::now().to_rfc3339()),
    }
}

fn observation_to_vector_result(
    obs: ClaudeMemObservation,
    idx: usize,
) -> frogclaw_core::vector_store::VectorSearchResult {
    let id = value_to_id(&obs.id);
    frogclaw_core::vector_store::VectorSearchResult {
        id: format!("claude-mem-{id}"),
        document_id: id,
        chunk_index: idx as i32,
        content: format_observation_content(&obs),
        score: obs.score.or(obs.rank).unwrap_or(idx as f32),
        rerank_score: None,
        has_embedding: true,
    }
}

fn recent_memory_results(
    query: &str,
    items: Vec<MemoryItem>,
    limit: usize,
) -> Vec<frogclaw_core::vector_store::VectorSearchResult> {
    let limit = limit.max(1);
    let prefer_recent = prefers_recent_context(query);
    let mut useful = items
        .into_iter()
        .filter(|item| {
            let content = format!("{}\n{}", item.title, item.content);
            !is_low_value_memory(&content)
        })
        .enumerate()
        .map(|(idx, item)| {
            let content = format!("{}\n{}", item.title.trim(), item.content.trim())
                .trim()
                .to_string();
            frogclaw_core::vector_store::VectorSearchResult {
                id: format!("claude-mem-recent-{}", item.id),
                document_id: item.id,
                chunk_index: idx as i32,
                content,
                score: if prefer_recent {
                    idx as f32
                } else {
                    1000.0 + idx as f32
                },
                rerank_score: None,
                has_embedding: false,
            }
        })
        .collect::<Vec<_>>();
    useful.truncate(limit);
    useful
}

fn filter_meaningful_memory_results(
    results: Vec<frogclaw_core::vector_store::VectorSearchResult>,
) -> Vec<frogclaw_core::vector_store::VectorSearchResult> {
    results
        .into_iter()
        .filter(|result| !is_low_value_memory(&result.content))
        .collect()
}

fn merge_memory_results(
    first: Vec<frogclaw_core::vector_store::VectorSearchResult>,
    second: Vec<frogclaw_core::vector_store::VectorSearchResult>,
    limit: usize,
) -> Vec<frogclaw_core::vector_store::VectorSearchResult> {
    let mut merged = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for result in first.into_iter().chain(second) {
        if result.content.trim().is_empty() || !seen.insert(result.document_id.clone()) {
            continue;
        }
        merged.push(result);
        if merged.len() >= limit {
            break;
        }
    }
    merged
}

fn prefers_recent_context(query: &str) -> bool {
    let lowered = query.to_lowercase();
    [
        "刚才",
        "刚刚",
        "之前",
        "上次",
        "前面",
        "最近",
        "刚写",
        "写的诗",
        "那首诗",
        "poem",
        "previous",
        "last time",
        "earlier",
        "recent",
    ]
    .iter()
    .any(|needle| lowered.contains(needle))
}

fn is_low_value_memory(content: &str) -> bool {
    let normalized = content.trim().to_lowercase();
    if normalized.is_empty() {
        return true;
    }
    if [
        "no previous sessions found",
        "no previous sessions found for this project",
        "当前项目记忆里也没有",
        "当前项目记忆里也显示没有",
        "没有可用的历史会话",
        "没有可用的上一轮",
        "没有保存到先前会话",
        "no available memory",
        "no saved memory",
    ]
    .iter()
    .any(|needle| normalized.contains(needle))
    {
        return true;
    }
    let negative_markers = [
        "看不到",
        "找不到",
        "未找到",
        "没有找到",
        "召回不到",
        "could not find",
        "can't find",
        "cannot find",
        "unable to find",
        "not found",
    ];
    let recall_markers = [
        "刚才",
        "刚刚",
        "之前",
        "上次",
        "前面",
        "历史会话",
        "当前对话",
        "上一轮",
        "项目记忆",
        "写的诗",
        "那首诗",
        "previous session",
        "previous conversation",
        "earlier conversation",
        "last session",
        "last time",
        "poem",
        "memory",
    ];
    negative_markers
        .iter()
        .any(|needle| normalized.contains(needle))
        && recall_markers
            .iter()
            .any(|needle| normalized.contains(needle))
}

fn is_useful_injected_context(context: &str) -> bool {
    let trimmed = context.trim();
    !trimmed.is_empty() && !is_low_value_memory(trimmed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn memory_item(id: &str, title: &str, content: &str) -> MemoryItem {
        MemoryItem {
            id: id.to_string(),
            namespace_id: DEFAULT_NAMESPACE_ID.to_string(),
            title: title.to_string(),
            content: content.to_string(),
            source: "manual".to_string(),
            index_status: "ready".to_string(),
            index_error: None,
            updated_at: "2026-05-17T00:00:00Z".to_string(),
        }
    }

    #[test]
    fn injected_placeholder_is_not_useful_context() {
        assert!(!is_useful_injected_context(
            "# [workspace] recent context\nNo previous sessions found."
        ));
    }

    #[test]
    fn recent_fallback_keeps_poem_and_filters_failed_lookup() {
        let results = recent_memory_results(
            "刚才写的诗是什么",
            vec![
                memory_item(
                    "18",
                    "FrogClaw 会话记忆：写一首诗",
                    "Assistant: 夜雨轻敲旧窗台，\n一灯微暖照尘埃。",
                ),
                memory_item(
                    "17",
                    "FrogClaw 会话记忆：刚才写的诗是什么",
                    "Assistant: 我这边看不到“刚才写的诗”的内容。",
                ),
            ],
            5,
        );

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].document_id, "18");
        assert!(results[0].content.contains("夜雨轻敲旧窗台"));
    }

    #[test]
    fn low_value_filter_does_not_drop_general_project_fixes() {
        assert!(!is_low_value_memory(
            "修复 Windows 下找不到配置文件的问题：改用 .frogclaw 路径解析。"
        ));
    }
}

fn context_to_rag_result(context: String, count: usize) -> RagContextResult {
    RagContextResult {
        context_parts: vec![format!("[Claude-Mem Project Memory]\n{}", context.trim())],
        source_results: vec![RagSourceResult {
            source_type: "memory".to_string(),
            container_id: DEFAULT_NAMESPACE_ID.to_string(),
            items: vec![RagRetrievedItem {
                content: context,
                score: 0.0,
                rerank_score: None,
                document_id: DEFAULT_NAMESPACE_ID.to_string(),
                id: format!("claude-mem-context-{count}"),
                document_name: Some(DEFAULT_NAMESPACE_NAME.to_string()),
            }],
        }],
    }
}

fn vector_results_to_rag_result(
    results: Vec<frogclaw_core::vector_store::VectorSearchResult>,
) -> RagContextResult {
    if results.is_empty() {
        return RagContextResult {
            context_parts: vec![],
            source_results: vec![],
        };
    }
    let context_parts = vec![format!(
        "[Claude-Mem Search Results]\n{}",
        results
            .iter()
            .map(|r| r.content.trim())
            .filter(|v| !v.is_empty())
            .collect::<Vec<_>>()
            .join("\n\n")
    )];
    let items = results
        .into_iter()
        .map(|r| RagRetrievedItem {
            content: r.content,
            score: r.score,
            rerank_score: r.rerank_score,
            document_id: r.document_id,
            id: r.id,
            document_name: Some(DEFAULT_NAMESPACE_NAME.to_string()),
        })
        .collect();
    RagContextResult {
        context_parts,
        source_results: vec![RagSourceResult {
            source_type: "memory".to_string(),
            container_id: DEFAULT_NAMESPACE_ID.to_string(),
            items,
        }],
    }
}

async fn start_managed_worker() -> Result<(), String> {
    append_memory_log("worker start_managed requested");
    reap_finished_worker().await;

    {
        let process = WORKER_PROCESS.get_or_init(|| Mutex::new(None)).lock().await;
        if process.is_some() {
            append_memory_log("worker start_managed skipped existing_child=true");
            return Ok(());
        }
    }

    let runtime_config = {
        let guard = RUNTIME_CONFIG.get_or_init(|| Mutex::new(None)).lock().await;
        guard.clone().unwrap_or_else(make_default_claude_config)
    };
    ensure_claude_mem_config(&runtime_config)?;
    append_memory_log(format!(
        "worker config_ready provider={} provider_id={} model={} settings_path={} env_file={}",
        runtime_config.provider.label(),
        runtime_config.provider.provider_id().unwrap_or("-"),
        runtime_config.provider.model_id().unwrap_or("-"),
        compact_log_value(&claude_mem_settings_file().display().to_string(), 220),
        compact_log_value(&claude_mem_env_file().display().to_string(), 220)
    ));

    let Some(start_command) = resolve_start_command() else {
        let err = "claude-mem executable/scripts not found. Set FROGCLAW_CLAUDE_MEM_HOME or put claude-mem under E:\\frogclaw\\claude-mem.".to_string();
        append_memory_log(format!(
            "worker start_managed failed error={}",
            compact_log_value(&err, 240)
        ));
        return Err(err);
    };

    append_memory_log(format!(
        "worker spawn program={} cwd={} args={}",
        compact_log_value(&start_command.program.display().to_string(), 220),
        compact_log_value(&start_command.cwd.display().to_string(), 220),
        compact_log_value(&start_command.args.join(" "), 160)
    ));
    let mut command = Command::new(&start_command.program);
    command
        .args(&start_command.args)
        .current_dir(&start_command.cwd);
    for (key, value) in &runtime_config.process_env {
        command.env(key, value);
    }
    if let Some(plugin_root) = start_command.plugin_root.as_ref() {
        command.env("CLAUDE_PLUGIN_ROOT", plugin_root);
        command.env("PLUGIN_ROOT", plugin_root);
    }
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let child = command.spawn().map_err(|e| {
        let err = format!("Failed to start claude-mem worker: {e}");
        append_memory_log(format!(
            "worker spawn failed error={}",
            compact_log_value(&err, 240)
        ));
        err
    })?;
    tracing::info!(
        "Started managed claude-mem worker pid {} from {}",
        child.id(),
        start_command.program.display()
    );
    append_memory_log(format!(
        "worker spawn ok pid={} program={}",
        child.id(),
        compact_log_value(&start_command.program.display().to_string(), 220)
    ));
    let mut process = WORKER_PROCESS.get_or_init(|| Mutex::new(None)).lock().await;
    *process = Some(child);
    Ok(())
}

async fn restart_managed_worker() -> Result<(), String> {
    append_memory_log("worker restart requested");
    stop_managed_worker().await;
    let result = start_managed_worker().await;
    if let Err(err) = &result {
        append_memory_log(format!(
            "worker restart failed error={}",
            compact_log_value(err, 240)
        ));
    } else {
        append_memory_log("worker restart ok");
    }
    result
}

async fn stop_managed_worker() {
    append_memory_log("worker stop requested");
    let mut process = WORKER_PROCESS.get_or_init(|| Mutex::new(None)).lock().await;
    if let Some(mut child) = process.take() {
        let pid = child.id();
        tracing::info!("Stopping managed claude-mem worker pid {}", pid);
        append_memory_log(format!("worker stop killing pid={}", pid));
        kill_process_tree(pid, &mut child);
        let _ = child.wait();
        append_memory_log(format!("worker stop done pid={}", pid));
    } else {
        append_memory_log("worker stop skipped no_child=true");
    }
}

fn kill_process_tree(pid: u32, child: &mut Child) {
    #[cfg(target_os = "windows")]
    {
        let status = Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        if status.map(|s| s.success()).unwrap_or(false) {
            return;
        }
    }
    let _ = child.kill();
}

async fn cleanup_leftover_worker_if_unmanaged() {
    let process = WORKER_PROCESS.get_or_init(|| Mutex::new(None)).lock().await;
    if process.is_some() {
        return;
    }
    drop(process);

    let Some(pid) = read_worker_pid_file() else {
        return;
    };
    if pid == std::process::id() {
        return;
    }
    if !is_pid_running(pid) {
        let _ = std::fs::remove_file(worker_pid_file());
        return;
    }

    tracing::info!(
        "Stopping leftover unmanaged claude-mem worker from previous FrogClaw run pid {}",
        pid
    );
    append_memory_log(format!("worker cleanup_leftover killing pid={}", pid));
    kill_pid_tree(pid);
    let _ = std::fs::remove_file(worker_pid_file());
    tokio::time::sleep(Duration::from_millis(500)).await;
    append_memory_log(format!("worker cleanup_leftover done pid={}", pid));
}

fn claude_mem_data_dir() -> PathBuf {
    std::env::var("CLAUDE_MEM_DATA_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| crate::paths::frogclaw_home().join("claude-mem"))
}

fn claude_mem_settings_file() -> PathBuf {
    claude_mem_data_dir().join("settings.json")
}

fn claude_mem_env_file() -> PathBuf {
    claude_mem_data_dir().join(".env")
}

fn ensure_claude_mem_config(config: &ClaudeMemRuntimeConfig) -> Result<(), String> {
    let data_dir = claude_mem_data_dir();
    std::fs::create_dir_all(&data_dir).map_err(|e| {
        format!(
            "创建 claude-mem 配置目录失败 path={} error={e}",
            data_dir.display()
        )
    })?;

    let settings_path = claude_mem_settings_file();
    let mut settings = read_settings_map(&settings_path)?;
    for (key, value) in &config.settings {
        settings.insert(key.clone(), value.clone());
    }
    write_settings_map(&settings_path, &settings)?;

    let env_path = claude_mem_env_file();
    let mut env_values = read_env_map(&env_path)?;
    for key in managed_claude_mem_env_keys() {
        if !config.env_file.contains_key(*key) {
            env_values.remove(*key);
        }
    }
    for (key, value) in &config.env_file {
        if value.is_empty() {
            env_values.remove(key);
        } else {
            env_values.insert(key.clone(), value.clone());
        }
    }
    write_env_map(&env_path, &env_values)?;
    append_memory_log(format!(
        "runtime_config files_written settings={} env_file={} provider={}",
        compact_log_value(&settings_path.display().to_string(), 220),
        compact_log_value(&env_path.display().to_string(), 220),
        config.provider.label()
    ));
    Ok(())
}

fn read_settings_map(path: &Path) -> Result<BTreeMap<String, String>, String> {
    if !path.exists() {
        return Ok(BTreeMap::new());
    }
    let raw = std::fs::read_to_string(path)
        .map_err(|e| format!("读取 claude-mem settings.json 失败: {e}"))?;
    let value: Value = serde_json::from_str(&raw)
        .map_err(|e| format!("解析 claude-mem settings.json 失败: {e}"))?;
    let source = value
        .get("env")
        .and_then(|env| env.as_object())
        .or_else(|| value.as_object())
        .ok_or_else(|| "claude-mem settings.json 不是 JSON 对象".to_string())?;
    let mut result = BTreeMap::new();
    for (key, value) in source {
        let as_string = value
            .as_str()
            .map(ToString::to_string)
            .unwrap_or_else(|| value.to_string());
        result.insert(key.clone(), as_string);
    }
    Ok(result)
}

fn write_settings_map(path: &Path, settings: &BTreeMap<String, String>) -> Result<(), String> {
    let raw = serde_json::to_string_pretty(settings)
        .map_err(|e| format!("序列化 claude-mem settings.json 失败: {e}"))?;
    atomic_write_text(path, &(raw + "\n"))
}

fn managed_claude_mem_env_keys() -> &'static [&'static str] {
    &[
        "ANTHROPIC_API_KEY",
        "ANTHROPIC_BASE_URL",
        "ANTHROPIC_AUTH_TOKEN",
        "GEMINI_API_KEY",
        "OPENROUTER_API_KEY",
    ]
}

fn read_env_map(path: &Path) -> Result<BTreeMap<String, String>, String> {
    if !path.exists() {
        return Ok(BTreeMap::new());
    }
    let raw =
        std::fs::read_to_string(path).map_err(|e| format!("读取 claude-mem .env 失败: {e}"))?;
    let mut result = BTreeMap::new();
    for line in raw.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let Some((key, value)) = trimmed.split_once('=') else {
            continue;
        };
        let value = value.trim();
        let unquoted = if (value.starts_with('"') && value.ends_with('"'))
            || (value.starts_with('\'') && value.ends_with('\''))
        {
            &value[1..value.len().saturating_sub(1)]
        } else {
            value
        };
        result.insert(key.trim().to_string(), unquoted.to_string());
    }
    Ok(result)
}

fn write_env_map(path: &Path, env_values: &BTreeMap<String, String>) -> Result<(), String> {
    let mut lines = vec![
        "# claude-mem credentials".to_string(),
        "# 自动由 FrogClaw 生成；不要把这个文件提交到版本库。".to_string(),
        String::new(),
    ];
    for (key, value) in env_values {
        if value.is_empty() {
            continue;
        }
        lines.push(format!("{}={}", key, format_env_value(value)));
    }
    atomic_write_text(path, &(lines.join("\n") + "\n"))?;
    set_owner_only_permissions(path);
    Ok(())
}

fn format_env_value(value: &str) -> String {
    if value
        .chars()
        .any(|ch| ch.is_whitespace() || ch == '#' || ch == '=' || ch == '"')
    {
        format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
    } else {
        value.to_string()
    }
}

fn atomic_write_text(path: &Path, content: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("创建目录失败 path={} error={e}", parent.display()))?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, content)
        .map_err(|e| format!("写入临时文件失败 path={} error={e}", tmp.display()))?;
    std::fs::rename(&tmp, path)
        .map_err(|e| format!("替换文件失败 path={} error={e}", path.display()))?;
    Ok(())
}

#[cfg(unix)]
fn set_owner_only_permissions(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
}

#[cfg(not(unix))]
fn set_owner_only_permissions(_path: &Path) {}

fn worker_pid_file() -> PathBuf {
    claude_mem_data_dir().join("worker.pid")
}

fn read_worker_pid_file() -> Option<u32> {
    let raw = std::fs::read_to_string(worker_pid_file()).ok()?;
    let value: Value = serde_json::from_str(&raw).ok()?;
    value
        .get("pid")
        .and_then(|pid| pid.as_u64())
        .and_then(|pid| u32::try_from(pid).ok())
}

fn is_pid_running(pid: u32) -> bool {
    #[cfg(target_os = "windows")]
    {
        Command::new("tasklist")
            .args(["/FI", &format!("PID eq {pid}")])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .output()
            .map(|output| {
                output.status.success()
                    && String::from_utf8_lossy(&output.stdout).contains(&pid.to_string())
            })
            .unwrap_or(false)
    }
    #[cfg(not(target_os = "windows"))]
    {
        Command::new("kill")
            .args(["-0", &pid.to_string()])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|status| status.success())
            .unwrap_or(false)
    }
}

fn kill_pid_tree(pid: u32) {
    #[cfg(target_os = "windows")]
    {
        let _ = Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = Command::new("kill")
            .args(["-TERM", &pid.to_string()])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

async fn reap_finished_worker() {
    let mut process = WORKER_PROCESS.get_or_init(|| Mutex::new(None)).lock().await;
    if let Some(child) = process.as_mut() {
        match child.try_wait() {
            Ok(Some(status)) => {
                tracing::warn!("Managed claude-mem worker exited with {}", status);
                append_memory_log(format!("worker exited status={}", status));
                *process = None;
            }
            Ok(None) => {}
            Err(err) => {
                tracing::warn!("Failed to poll managed claude-mem worker: {}", err);
                append_memory_log(format!(
                    "worker poll failed error={}",
                    compact_log_value(&err.to_string(), 240)
                ));
                *process = None;
            }
        }
    }
}

struct StartCommand {
    program: PathBuf,
    args: Vec<String>,
    cwd: PathBuf,
    plugin_root: Option<PathBuf>,
}

fn resolve_start_command() -> Option<StartCommand> {
    if let Ok(path) = std::env::var("FROGCLAW_CLAUDE_MEM_EXE") {
        let path = PathBuf::from(path);
        if path.is_file() {
            let cwd = path
                .parent()
                .map(Path::to_path_buf)
                .or_else(claude_mem_home)
                .unwrap_or_else(|| PathBuf::from("."));
            return Some(StartCommand {
                program: path,
                args: vec!["--daemon".to_string()],
                cwd,
                plugin_root: claude_mem_home().map(|home| home.join("plugin")),
            });
        }
    }

    if let Some(path) = packaged_claude_mem_exe() {
        let packaged_root = packaged_claude_mem_root(&path);
        let cwd = packaged_root
            .clone()
            .or_else(|| path.parent().map(Path::to_path_buf))
            .unwrap_or_else(|| PathBuf::from("."));
        let plugin_root = packaged_root
            .as_ref()
            .map(|root| root.join("plugin"))
            .or_else(|| claude_mem_home().map(|home| home.join("plugin")));
        return Some(StartCommand {
            program: path,
            args: vec!["--daemon".to_string()],
            cwd,
            plugin_root,
        });
    }

    let home = claude_mem_home()?;
    let exe_names: &[&str] = if cfg!(target_os = "windows") {
        &["claude-mem.exe", "claude-mem"]
    } else {
        &["claude-mem"]
    };
    for name in exe_names {
        for path in [
            home.join(name),
            home.join("plugin").join("scripts").join(name),
            home.join("dist").join("npx-cli").join(name),
            home.join("dist").join("binaries").join(name),
        ] {
            if path.is_file() {
                return Some(StartCommand {
                    program: path,
                    args: vec!["--daemon".to_string()],
                    cwd: home.clone(),
                    plugin_root: Some(home.join("plugin")),
                });
            }
        }
    }
    if let Some(path) = find_worker_binary(&home) {
        return Some(StartCommand {
            program: path,
            args: vec!["--daemon".to_string()],
            cwd: home.clone(),
            plugin_root: Some(home.join("plugin")),
        });
    }

    let worker = home
        .join("plugin")
        .join("scripts")
        .join("worker-service.cjs");
    if worker.is_file() {
        let bun = resolve_bun()?;
        return Some(StartCommand {
            program: bun,
            args: vec![worker.to_string_lossy().to_string(), "--daemon".to_string()],
            cwd: home.clone(),
            plugin_root: Some(home.join("plugin")),
        });
    }
    None
}

fn packaged_claude_mem_exe() -> Option<PathBuf> {
    let names: &[&str] = if cfg!(target_os = "windows") {
        &["claude-mem.exe"]
    } else {
        &["claude-mem"]
    };
    let mut dirs = Vec::new();
    if let Some(resource_dir) = RESOURCE_DIR.get() {
        dirs.push(resource_dir.clone());
        dirs.push(resource_dir.join("binaries"));
    }
    if let Ok(current_exe) = std::env::current_exe() {
        if let Some(exe_dir) = current_exe.parent() {
            dirs.push(exe_dir.to_path_buf());
            dirs.push(exe_dir.join("binaries"));
        }
    }
    if let Ok(current_dir) = std::env::current_dir() {
        dirs.push(current_dir.join("src-tauri").join("binaries"));
        dirs.push(current_dir.join("binaries"));
    }
    dirs.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("binaries"));
    for dir in dirs {
        for name in names {
            let path = dir.join(name);
            if path.is_file() {
                return Some(path);
            }
        }
    }
    None
}

fn packaged_claude_mem_root(exe_path: &Path) -> Option<PathBuf> {
    let exe_dir = exe_path.parent()?;
    for root in [exe_dir, exe_dir.parent()?] {
        if root
            .join("plugin")
            .join("scripts")
            .join("worker-service.cjs")
            .is_file()
        {
            return Some(root.to_path_buf());
        }
    }
    None
}

fn find_worker_binary(home: &Path) -> Option<PathBuf> {
    let binaries_dir = home.join("dist").join("binaries");
    let entries = std::fs::read_dir(binaries_dir).ok()?;
    let mut candidates = entries
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| {
            let Some(name) = path.file_name().and_then(|v| v.to_str()) else {
                return false;
            };
            path.is_file()
                && name.starts_with("worker-service-")
                && (name.ends_with(".exe") || !cfg!(target_os = "windows"))
        })
        .collect::<Vec<_>>();
    candidates.sort();
    candidates.pop()
}

fn claude_mem_home() -> Option<PathBuf> {
    if let Ok(path) = std::env::var("FROGCLAW_CLAUDE_MEM_HOME") {
        let path = PathBuf::from(path);
        if path.exists() {
            return Some(path);
        }
    }
    let candidates = [
        PathBuf::from(r"E:\frogclaw\claude-mem"),
        PathBuf::from(r"..\claude-mem"),
        std::env::current_dir().ok()?.join("claude-mem"),
    ];
    candidates.into_iter().find(|p| p.exists())
}

fn resolve_bun() -> Option<PathBuf> {
    if let Ok(path) = std::env::var("BUN_EXE") {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Some(path);
        }
    }
    let names: &[&str] = if cfg!(target_os = "windows") {
        &["bun.exe", "bun.cmd", "bun"]
    } else {
        &["bun"]
    };
    let path_var = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path_var) {
        for name in names {
            let candidate = dir.join(name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    if let Some(home) = dirs::home_dir() {
        let candidate = if cfg!(target_os = "windows") {
            home.join(".bun").join("bin").join("bun.exe")
        } else {
            home.join(".bun").join("bin").join("bun")
        };
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

fn obs_content(obs: &ClaudeMemObservation) -> String {
    obs.narrative
        .clone()
        .or_else(|| obs.text.clone())
        .or_else(|| obs.subtitle.clone())
        .unwrap_or_default()
}

fn format_observation_content(obs: &ClaudeMemObservation) -> String {
    let title = obs
        .title
        .clone()
        .unwrap_or_else(|| title_from_text(&obs_content(obs)));
    let kind = obs.r#type.clone().unwrap_or_else(|| "memory".to_string());
    let content = obs_content(obs);
    if content.trim().is_empty() {
        format!("{title} ({kind})")
    } else {
        format!("{title} ({kind})\n{content}")
    }
}

fn value_to_id(value: &Value) -> String {
    if let Some(s) = value.as_str() {
        return s.to_string();
    }
    if let Some(i) = value.as_i64() {
        return i.to_string();
    }
    if let Some(u) = value.as_u64() {
        return u.to_string();
    }
    frogclaw_core::utils::gen_id()
}

fn title_from_text(text: &str) -> String {
    let trimmed = text.trim();
    let title: String = trimmed.chars().take(60).collect();
    if title.is_empty() {
        "Claude-Mem Memory".to_string()
    } else if trimmed.chars().count() > 60 {
        format!("{title}...")
    } else {
        title
    }
}

fn compact_log_value(value: &str, max_chars: usize) -> String {
    let cleaned = value
        .replace('\r', "\\r")
        .replace('\n', "\\n")
        .replace('\t', "\\t");
    if cleaned.chars().count() <= max_chars {
        cleaned
    } else {
        format!(
            "{}... <truncated>",
            cleaned.chars().take(max_chars).collect::<String>()
        )
    }
}

fn normalize_project_path(value: &str) -> String {
    value
        .trim()
        .replace('\\', "/")
        .trim_end_matches('/')
        .to_string()
}

fn fallback_project_name(project_path: &str) -> String {
    normalize_project_path(project_path)
        .rsplit('/')
        .find(|part| !part.is_empty())
        .unwrap_or("frogclaw")
        .to_string()
}
