use frogclaw_core::types::{
    MemoryItem, MemoryNamespace, ProjectMemoryProfile, RagContextResult, RagRetrievedItem,
    RagSourceResult,
};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::time::Duration;
use tokio::sync::Mutex;

const DEFAULT_BASE_URL: &str = "http://127.0.0.1:37777";
const DEFAULT_NAMESPACE_ID: &str = "claude-mem";
const DEFAULT_NAMESPACE_NAME: &str = "Claude-Mem";
static START_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

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
            .timeout(Duration::from_secs(15))
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
        if self.health().await.is_ok() {
            return Ok(());
        }

        let _guard = START_LOCK
            .get_or_init(|| Mutex::new(()))
            .lock()
            .await;
        if self.health().await.is_ok() {
            return Ok(());
        }

        start_local_worker().await?;
        let start = std::time::Instant::now();
        while start.elapsed() < Duration::from_secs(20) {
            if self.health().await.is_ok() {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(400)).await;
        }
        Err("claude-mem worker did not become ready on http://127.0.0.1:37777".to_string())
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
        self.ensure_ready().await?;
        let request = SaveMemoryRequest {
            text: input.text.trim(),
            title: input.title.as_deref().filter(|v| !v.trim().is_empty()),
            project: input.project.as_deref().filter(|v| !v.trim().is_empty()),
            metadata: input.metadata.as_ref(),
        };
        let response = self
            .client
            .post(format!("{}/api/memory/save", self.base_url))
            .json(&request)
            .send()
            .await
            .map_err(|e| format!("claude-mem save failed: {e}"))?;
        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(format!("claude-mem save returned {status}: {body}"));
        }
        let saved: SaveMemoryResponse = response
            .json()
            .await
            .map_err(|e| format!("claude-mem save response parse failed: {e}"))?;
        Ok(MemoryItem {
            id: value_to_id(&saved.id),
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

    pub async fn list_items(
        &self,
        project: Option<&str>,
        limit: usize,
    ) -> Result<Vec<MemoryItem>, String> {
        self.ensure_ready().await?;
        let mut request = self
            .client
            .get(format!("{}/api/observations", self.base_url))
            .query(&[("offset", "0"), ("limit", &limit.to_string())]);
        if let Some(project) = project.filter(|v| !v.trim().is_empty()) {
            request = request.query(&[("project", project)]);
        }
        let response = request
            .send()
            .await
            .map_err(|e| format!("claude-mem list failed: {e}"))?;
        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(format!("claude-mem list returned {status}: {body}"));
        }
        let body: Value = response
            .json()
            .await
            .map_err(|e| format!("claude-mem list response parse failed: {e}"))?;
        Ok(parse_observation_list(body)
            .into_iter()
            .map(observation_to_item)
            .collect())
    }

    pub async fn search_memory(
        &self,
        query: &str,
        project: Option<&str>,
        limit: usize,
    ) -> Result<Vec<frogclaw_core::vector_store::VectorSearchResult>, String> {
        self.ensure_ready().await?;
        let mut request = self.client.get(format!("{}/api/search", self.base_url)).query(&[
            ("query", query),
            ("searchType", "observations"),
            ("format", "json"),
            ("limit", &limit.to_string()),
        ]);
        if let Some(project) = project.filter(|v| !v.trim().is_empty()) {
            request = request.query(&[("project", project)]);
        }
        let response = request
            .send()
            .await
            .map_err(|e| format!("claude-mem search failed: {e}"))?;
        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(format!("claude-mem search returned {status}: {body}"));
        }
        let body: Value = response
            .json()
            .await
            .map_err(|e| format!("claude-mem search response parse failed: {e}"))?;
        let observations = parse_search_observations(body);
        Ok(observations
            .into_iter()
            .take(limit)
            .enumerate()
            .map(|(idx, obs)| observation_to_vector_result(obs, idx))
            .collect())
    }

    pub async fn collect_context(
        &self,
        query: &str,
        project: Option<&str>,
        limit: usize,
    ) -> Result<RagContextResult, String> {
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
            .map_err(|e| format!("claude-mem semantic context failed: {e}"))?;
        if semantic_response.status().is_success() {
            let body: ContextResponse = semantic_response
                .json()
                .await
                .map_err(|e| format!("claude-mem semantic context response parse failed: {e}"))?;
            let context = body.context.unwrap_or_default();
            if !context.trim().is_empty() {
                return Ok(context_to_rag_result(
                    context,
                    body.count.unwrap_or(1).max(1),
                ));
            }
        } else {
            tracing::warn!(
                "claude-mem semantic context returned {}. Falling back to inject/search.",
                semantic_response.status()
            );
        }

        let mut request = self
            .client
            .get(format!("{}/api/context/inject", self.base_url))
            .query(&[("limit", limit.to_string()), ("full", "true".to_string())]);
        if let Some(project) = project.filter(|v| !v.trim().is_empty()) {
            request = request.query(&[("projects", project)]);
        }
        let response = request
            .send()
            .await
            .map_err(|e| format!("claude-mem context failed: {e}"))?;
        if response.status().is_success() {
            let context = response
                .text()
                .await
                .map_err(|e| format!("claude-mem context response read failed: {e}"))?;
            if !context.trim().is_empty() {
                return Ok(context_to_rag_result(context, 1));
            }
        } else {
            tracing::warn!(
                "claude-mem context returned {}. Falling back to search.",
                response.status()
            );
        }

        let results = self.search_memory(query, project, limit).await?;
        Ok(vector_results_to_rag_result(results))
    }
}

pub async fn save_auto_memory(
    project_path: Option<&str>,
    project_name: Option<&str>,
    title: &str,
    text: &str,
    source: &str,
) -> Result<MemoryItem, String> {
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
    let client = ClaudeMemClient::new()?;
    let project = project_name
        .filter(|v| !v.trim().is_empty())
        .or_else(|| project_path.and_then(|p| Path::new(p).file_name().and_then(|v| v.to_str())));
    client.collect_context(query, project, limit).await
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
        .or_else(|| body.get("results").and_then(|v| v.get("observations")).cloned())
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default()
}

fn observation_to_item(obs: ClaudeMemObservation) -> MemoryItem {
    let content = obs_content(&obs);
    MemoryItem {
        id: value_to_id(&obs.id),
        namespace_id: DEFAULT_NAMESPACE_ID.to_string(),
        title: obs.title.clone().unwrap_or_else(|| title_from_text(&content)),
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
        context_parts: vec![format!(
            "[Claude-Mem Project Memory]\n{}",
            context.trim()
        )],
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

async fn start_local_worker() -> Result<(), String> {
    let Some(start_command) = resolve_start_command() else {
        return Err("claude-mem executable/scripts not found. Set FROGCLAW_CLAUDE_MEM_HOME or put claude-mem under E:\\frogclaw\\claude-mem.".to_string());
    };

    let mut command = Command::new(&start_command.program);
    command
        .args(&start_command.args)
        .current_dir(&start_command.cwd)
        .env("CLAUDE_MEM_WORKER_HOST", "127.0.0.1")
        .env("CLAUDE_MEM_WORKER_PORT", "37777")
        .env(
            "CLAUDE_MEM_DATA_DIR",
            std::env::var("CLAUDE_MEM_DATA_DIR").unwrap_or_else(|_| {
                dirs::home_dir()
                    .unwrap_or_else(|| PathBuf::from("."))
                    .join(".frogclaw")
                    .join("claude-mem")
                    .to_string_lossy()
                    .to_string()
            }),
        )
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    command
        .spawn()
        .map_err(|e| format!("Failed to start claude-mem worker: {e}"))?;
    Ok(())
}

struct StartCommand {
    program: PathBuf,
    args: Vec<String>,
    cwd: PathBuf,
}

fn resolve_start_command() -> Option<StartCommand> {
    let home = claude_mem_home()?;
    if let Ok(path) = std::env::var("FROGCLAW_CLAUDE_MEM_EXE") {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Some(StartCommand {
                program: path,
                args: vec!["start".to_string()],
                cwd: home,
            });
        }
    }

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
                    args: vec!["start".to_string()],
                    cwd: home,
                });
            }
        }
    }
    if let Some(path) = find_worker_binary(&home) {
        return Some(StartCommand {
            program: path,
            args: vec!["start".to_string()],
            cwd: home,
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
            args: vec![worker.to_string_lossy().to_string(), "start".to_string()],
            cwd: home,
        });
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
    let title = obs.title.clone().unwrap_or_else(|| title_from_text(&obs_content(obs)));
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

fn normalize_project_path(value: &str) -> String {
    value.trim().replace('\\', "/").trim_end_matches('/').to_string()
}

fn fallback_project_name(project_path: &str) -> String {
    normalize_project_path(project_path)
        .rsplit('/')
        .find(|part| !part.is_empty())
        .unwrap_or("frogclaw")
        .to_string()
}
