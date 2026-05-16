use frogclaw_core::types::{
    MemoryItem, MemoryNamespace, ProjectMemoryProfile, RagContextResult, RagRetrievedItem,
    RagSourceResult,
};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::OnceLock;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

const DEFAULT_BASE_URL: &str = "http://127.0.0.1:37777";
const DEFAULT_NAMESPACE_ID: &str = "claude-mem";
const DEFAULT_NAMESPACE_NAME: &str = "Claude-Mem";
static START_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
static WORKER_PROCESS: OnceLock<Mutex<Option<Child>>> = OnceLock::new();
static RESOURCE_DIR: OnceLock<PathBuf> = OnceLock::new();
static SHUTTING_DOWN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

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
        let response = request.send().await.map_err(|e| {
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
        let response = request.send().await.map_err(|e| {
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
            if !context.trim().is_empty() {
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
            tracing::warn!(
                "claude-mem context returned {}. Falling back to search.",
                response.status()
            );
            append_memory_log(format!(
                "collect_context inject_status status={} fallback=search",
                response.status()
            ));
        }

        let results = self.search_memory(query, project, limit).await?;
        append_memory_log(format!(
            "collect_context ok source=search count={} elapsed_ms={}",
            results.len(),
            started.elapsed().as_millis()
        ));
        Ok(vector_results_to_rag_result(results))
    }
}

pub fn init_resource_dir(path: PathBuf) {
    let _ = RESOURCE_DIR.set(path);
}

pub fn start_background_worker() {
    append_memory_log("background_worker start requested");
    tauri::async_runtime::spawn(async {
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
    let line = compact_log_value(message.as_ref(), 900);
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
        .current_dir(&start_command.cwd)
        .env("CLAUDE_MEM_WORKER_HOST", "127.0.0.1")
        .env("CLAUDE_MEM_WORKER_PORT", "37777")
        .env("CLAUDE_MEM_DATA_DIR", claude_mem_data_dir());
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
