use crate::AppState;
use frogclaw_core::types::*;
use tauri::State;

fn default_project_path() -> String {
    crate::paths::default_workspace()
        .to_string_lossy()
        .to_string()
}

fn project_name_from_path(project_path: &str, project_name: Option<&str>) -> String {
    project_name
        .filter(|v| !v.trim().is_empty())
        .map(|v| v.trim().to_string())
        .unwrap_or_else(|| {
            std::path::Path::new(project_path)
                .file_name()
                .and_then(|v| v.to_str())
                .unwrap_or("frogclaw")
                .to_string()
        })
}

fn metadata_for_source(source: &str, project_path: Option<&str>) -> serde_json::Value {
    serde_json::json!({
        "app": "frogclaw",
        "source": source,
        "projectPath": project_path,
    })
}

fn log_memory_command(event: &str, detail: impl AsRef<str>) {
    crate::claude_mem::append_memory_log(format!("command {event} {}", detail.as_ref()));
}

#[tauri::command]
pub async fn list_memory_namespaces() -> Result<Vec<MemoryNamespace>, String> {
    log_memory_command("list_memory_namespaces", "start");
    Ok(vec![crate::claude_mem::ClaudeMemClient::namespace()])
}

#[tauri::command]
pub async fn list_project_memory_profiles(
    state: State<'_, AppState>,
) -> Result<Vec<ProjectMemoryProfile>, String> {
    log_memory_command("list_project_memory_profiles", "start");
    let default_project_path = default_project_path();
    let mut projects = vec![(
        default_project_path.clone(),
        project_name_from_path(&default_project_path, None),
    )];

    let conversations = frogclaw_core::repo::conversation::list_conversations(&state.sea_db)
        .await
        .unwrap_or_default();
    for conversation in conversations {
        let Some(path) = conversation.working_directory else {
            continue;
        };
        if path.trim().is_empty() || projects.iter().any(|(p, _)| p == &path) {
            continue;
        }
        let name = project_name_from_path(&path, conversation.project_name.as_deref());
        projects.push((path, name));
    }

    let profiles = projects
        .into_iter()
        .map(|(path, name)| crate::claude_mem::ClaudeMemClient::project_profile(&path, Some(&name)))
        .collect::<Vec<_>>();
    log_memory_command(
        "list_project_memory_profiles",
        format!("ok count={}", profiles.len()),
    );
    Ok(profiles)
}

#[tauri::command]
pub async fn get_project_memory_profile(
    project_path: String,
    project_name: Option<String>,
) -> Result<ProjectMemoryProfile, String> {
    log_memory_command(
        "get_project_memory_profile",
        format!(
            "start project_path_chars={} project_name={}",
            project_path.chars().count(),
            project_name.as_deref().unwrap_or("-")
        ),
    );
    Ok(crate::claude_mem::ClaudeMemClient::project_profile(
        &project_path,
        project_name.as_deref(),
    ))
}

#[tauri::command]
pub async fn update_project_memory_profile(
    project_path: String,
    project_name: Option<String>,
    _input: UpdateMemoryNamespaceInput,
) -> Result<ProjectMemoryProfile, String> {
    log_memory_command(
        "update_project_memory_profile",
        format!(
            "start project_path_chars={} project_name={}",
            project_path.chars().count(),
            project_name.as_deref().unwrap_or("-")
        ),
    );
    Ok(crate::claude_mem::ClaudeMemClient::project_profile(
        &project_path,
        project_name.as_deref(),
    ))
}

#[tauri::command]
pub async fn list_project_memory_items(
    project_path: String,
    project_name: Option<String>,
) -> Result<Vec<MemoryItem>, String> {
    log_memory_command(
        "list_project_memory_items",
        format!(
            "start project_path_chars={} project_name={}",
            project_path.chars().count(),
            project_name.as_deref().unwrap_or("-")
        ),
    );
    let client = crate::claude_mem::ClaudeMemClient::new()?;
    let project = project_name_from_path(&project_path, project_name.as_deref());
    client.list_items(Some(&project), 200).await
}

#[tauri::command]
pub async fn add_project_memory_item(
    project_path: String,
    project_name: Option<String>,
    title: String,
    content: String,
) -> Result<MemoryItem, String> {
    log_memory_command(
        "add_project_memory_item",
        format!(
            "start project_path_chars={} project_name={} title_chars={} content_chars={}",
            project_path.chars().count(),
            project_name.as_deref().unwrap_or("-"),
            title.chars().count(),
            content.chars().count()
        ),
    );
    let client = crate::claude_mem::ClaudeMemClient::new()?;
    let project = project_name_from_path(&project_path, project_name.as_deref());
    client
        .save_memory(crate::claude_mem::ClaudeMemSaveInput {
            title: Some(title),
            text: content,
            project: Some(project),
            metadata: Some(metadata_for_source("manual", Some(&project_path))),
        })
        .await
}

#[tauri::command]
pub async fn summarize_project_memory(
    state: State<'_, AppState>,
    project_path: String,
    project_name: Option<String>,
) -> Result<usize, String> {
    log_memory_command(
        "summarize_project_memory",
        format!(
            "start project_path_chars={} project_name={}",
            project_path.chars().count(),
            project_name.as_deref().unwrap_or("-")
        ),
    );
    let target = project_path
        .trim()
        .replace('\\', "/")
        .trim_end_matches('/')
        .to_lowercase();
    let mut conversations = frogclaw_core::repo::conversation::list_conversations(&state.sea_db)
        .await
        .map_err(|e| e.to_string())?
        .into_iter()
        .filter(|conversation| {
            conversation
                .working_directory
                .as_deref()
                .map(|p| {
                    p.trim()
                        .replace('\\', "/")
                        .trim_end_matches('/')
                        .to_lowercase()
                })
                .as_deref()
                == Some(target.as_str())
        })
        .collect::<Vec<_>>();
    conversations.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    if conversations.is_empty() {
        log_memory_command("summarize_project_memory", "failed reason=no_conversations");
        return Err("当前项目没有可总结的会话".to_string());
    }

    let mut saved = 0usize;
    let project = project_name_from_path(&project_path, project_name.as_deref());
    let client = crate::claude_mem::ClaudeMemClient::new()?;
    for conversation in conversations.iter().take(8) {
        log_memory_command(
            "summarize_project_memory",
            format!(
                "save_conversation conversation_id={} title_chars={}",
                conversation.id,
                conversation.title.chars().count()
            ),
        );
        let messages = frogclaw_core::repo::message::list_messages(&state.sea_db, &conversation.id)
            .await
            .map_err(|e| e.to_string())?;
        let transcript = messages
            .iter()
            .rev()
            .take(20)
            .rev()
            .filter_map(|msg| {
                let content = msg.content.trim();
                if content.is_empty() {
                    return None;
                }
                let role = match msg.role {
                    MessageRole::User => "User",
                    MessageRole::Assistant => "Assistant",
                    MessageRole::System => "System",
                    MessageRole::Tool => "Tool",
                };
                Some(format!("{role}: {content}"))
            })
            .collect::<Vec<_>>()
            .join("\n\n");
        if transcript.trim().is_empty() {
            continue;
        }
        let text = format!(
            "FrogClaw project conversation summary source.\nProject: {}\nConversation: {}\n\n{}",
            project, conversation.title, transcript
        );
        client
            .save_memory(crate::claude_mem::ClaudeMemSaveInput {
                title: Some(format!("FrogClaw 会话记忆：{}", conversation.title)),
                text,
                project: Some(project.clone()),
                metadata: Some(metadata_for_source("manual_summarize", Some(&project_path))),
            })
            .await?;
        saved += 1;
    }
    log_memory_command(
        "summarize_project_memory",
        format!("ok saved={} scanned={}", saved, conversations.len().min(8)),
    );
    Ok(saved)
}

#[tauri::command]
pub async fn create_memory_namespace(
    input: CreateMemoryNamespaceInput,
) -> Result<MemoryNamespace, String> {
    log_memory_command(
        "create_memory_namespace",
        format!(
            "start name_chars={} scope={}",
            input.name.chars().count(),
            input.scope
        ),
    );
    let mut ns = crate::claude_mem::ClaudeMemClient::namespace();
    if !input.name.trim().is_empty() {
        ns.name = input.name;
    }
    ns.scope = input.scope;
    Ok(ns)
}

#[tauri::command]
pub async fn delete_memory_namespace(_id: String) -> Result<(), String> {
    log_memory_command(
        "delete_memory_namespace",
        "failed reason=claude_mem_single_local_namespace",
    );
    Err("claude-mem 当前只提供单一本地记忆库，不支持从 FrogClaw 删除命名空间".to_string())
}

#[tauri::command]
pub async fn update_memory_namespace(
    _id: String,
    input: UpdateMemoryNamespaceInput,
) -> Result<MemoryNamespace, String> {
    log_memory_command(
        "update_memory_namespace",
        format!(
            "start id={} name_chars={}",
            _id,
            input
                .name
                .as_deref()
                .map(|v| v.chars().count())
                .unwrap_or(0)
        ),
    );
    let mut ns = crate::claude_mem::ClaudeMemClient::namespace();
    if let Some(name) = input.name.filter(|v| !v.trim().is_empty()) {
        ns.name = name;
    }
    if let Some(sort_order) = input.sort_order {
        ns.sort_order = sort_order;
    }
    Ok(ns)
}

#[tauri::command]
pub async fn list_memory_items(_namespace_id: String) -> Result<Vec<MemoryItem>, String> {
    log_memory_command(
        "list_memory_items",
        format!("start namespace_id={}", _namespace_id),
    );
    let client = crate::claude_mem::ClaudeMemClient::new()?;
    client.list_items(None, 200).await
}

#[tauri::command]
pub async fn add_memory_item(input: CreateMemoryItemInput) -> Result<MemoryItem, String> {
    log_memory_command(
        "add_memory_item",
        format!(
            "start namespace_id={} title_chars={} content_chars={}",
            input.namespace_id,
            input.title.chars().count(),
            input.content.chars().count()
        ),
    );
    let client = crate::claude_mem::ClaudeMemClient::new()?;
    client
        .save_memory(crate::claude_mem::ClaudeMemSaveInput {
            title: Some(input.title),
            text: input.content,
            project: None,
            metadata: Some(metadata_for_source(
                input.source.as_deref().unwrap_or("manual"),
                None,
            )),
        })
        .await
}

#[tauri::command]
pub async fn delete_memory_item(_namespace_id: String, _id: String) -> Result<(), String> {
    log_memory_command(
        "delete_memory_item",
        "failed reason=claude_mem_delete_api_unavailable",
    );
    Err("claude-mem 当前本地 worker 未提供删除单条记忆 API".to_string())
}

#[tauri::command]
pub async fn update_memory_item(
    _namespace_id: String,
    id: String,
    input: UpdateMemoryItemInput,
) -> Result<MemoryItem, String> {
    log_memory_command(
        "update_memory_item",
        format!(
            "start namespace_id={} item_id={} has_content={}",
            _namespace_id,
            id,
            input.content.is_some()
        ),
    );
    let title = input
        .title
        .unwrap_or_else(|| format!("Updated memory {id}"));
    let content = input
        .content
        .ok_or_else(|| "claude-mem update requires content; saved as a new memory".to_string())?;
    let client = crate::claude_mem::ClaudeMemClient::new()?;
    client
        .save_memory(crate::claude_mem::ClaudeMemSaveInput {
            title: Some(title),
            text: content,
            project: None,
            metadata: Some(metadata_for_source("manual_update", None)),
        })
        .await
}

#[tauri::command]
pub async fn search_memory(
    _namespace_id: String,
    query: String,
    top_k: Option<usize>,
) -> Result<Vec<frogclaw_core::vector_store::VectorSearchResult>, String> {
    log_memory_command(
        "search_memory",
        format!(
            "start namespace_id={} query_chars={} top_k={}",
            _namespace_id,
            query.chars().count(),
            top_k.unwrap_or(5)
        ),
    );
    let client = crate::claude_mem::ClaudeMemClient::new()?;
    client.search_memory(&query, None, top_k.unwrap_or(5)).await
}

#[tauri::command]
pub async fn search_project_memory(
    project_path: String,
    project_name: Option<String>,
    query: String,
    top_k: Option<usize>,
) -> Result<Vec<frogclaw_core::vector_store::VectorSearchResult>, String> {
    log_memory_command(
        "search_project_memory",
        format!(
            "start project_path_chars={} project_name={} query_chars={} top_k={}",
            project_path.chars().count(),
            project_name.as_deref().unwrap_or("-"),
            query.chars().count(),
            top_k.unwrap_or(8)
        ),
    );
    let client = crate::claude_mem::ClaudeMemClient::new()?;
    let project = project_name_from_path(&project_path, project_name.as_deref());
    client
        .search_memory(&query, Some(&project), top_k.unwrap_or(8))
        .await
}

#[tauri::command]
pub async fn rebuild_memory_index(_namespace_id: String) -> Result<(), String> {
    log_memory_command(
        "rebuild_memory_index",
        "failed reason=claude_mem_managed_index",
    );
    Err("claude-mem 自己维护索引，FrogClaw 不再重建内部记忆索引".to_string())
}

#[tauri::command]
pub async fn clear_memory_index(_namespace_id: String) -> Result<(), String> {
    log_memory_command(
        "clear_memory_index",
        "failed reason=claude_mem_managed_index",
    );
    Err("claude-mem 自己维护索引，FrogClaw 不再清空内部记忆索引".to_string())
}

#[tauri::command]
pub async fn reindex_memory_item(_namespace_id: String, _item_id: String) -> Result<(), String> {
    log_memory_command(
        "reindex_memory_item",
        "failed reason=claude_mem_managed_index",
    );
    Err("claude-mem 自己维护索引，FrogClaw 不再重建单条记忆索引".to_string())
}

#[tauri::command]
pub async fn reorder_memory_namespaces(_namespace_ids: Vec<String>) -> Result<(), String> {
    log_memory_command(
        "reorder_memory_namespaces",
        format!("ok count={}", _namespace_ids.len()),
    );
    Ok(())
}
