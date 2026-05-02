use crate::AppState;
use frogclaw_core::types::*;
use tauri::State;

#[tauri::command]
pub async fn list_mcp_servers(state: State<'_, AppState>) -> Result<Vec<McpServer>, String> {
    frogclaw_core::repo::mcp_server::list_mcp_servers(&state.sea_db)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn create_mcp_server(
    state: State<'_, AppState>,
    input: CreateMcpServerInput,
) -> Result<McpServer, String> {
    frogclaw_core::repo::mcp_server::create_mcp_server(&state.sea_db, input)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn update_mcp_server(
    state: State<'_, AppState>,
    id: String,
    input: CreateMcpServerInput,
) -> Result<McpServer, String> {
    frogclaw_core::repo::mcp_server::update_mcp_server(&state.sea_db, &id, input)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn delete_mcp_server(state: State<'_, AppState>, id: String) -> Result<(), String> {
    frogclaw_core::repo::mcp_server::delete_mcp_server(&state.sea_db, &id)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn test_mcp_server(
    _state: State<'_, AppState>,
    _id: String,
) -> Result<serde_json::Value, String> {
    // Mock implementation — return success with capabilities
    Ok(serde_json::json!({"ok": true, "capabilities": ["tools"]}))
}

#[tauri::command]
pub async fn list_mcp_tools(
    state: State<'_, AppState>,
    server_id: String,
) -> Result<Vec<ToolDescriptor>, String> {
    frogclaw_core::repo::mcp_server::list_tools_for_server(&state.sea_db, &server_id)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn discover_mcp_tools(
    state: State<'_, AppState>,
    id: String,
) -> Result<Vec<ToolDescriptor>, String> {
    let server = frogclaw_core::repo::mcp_server::get_mcp_server(&state.sea_db, &id)
        .await
        .map_err(|e| e.to_string())?;

    if server.source == "builtin" {
        return frogclaw_core::repo::mcp_server::list_tools_for_server(&state.sea_db, &id)
            .await
            .map_err(|e| e.to_string());
    }

    let timeout_secs = server.discover_timeout_secs.unwrap_or(30) as u64;
    let timeout_duration = std::time::Duration::from_secs(timeout_secs);

    let tools = match server.transport.as_str() {
        "stdio" => {
            let command = server
                .command
                .as_deref()
                .ok_or_else(|| "stdio server has no command configured".to_string())?;
            let args: Vec<String> = server
                .args_json
                .as_ref()
                .and_then(|s| serde_json::from_str(s).ok())
                .unwrap_or_default();
            let env: std::collections::HashMap<String, String> = server
                .env_json
                .as_ref()
                .and_then(|s| serde_json::from_str(s).ok())
                .unwrap_or_default();
            tokio::time::timeout(
                timeout_duration,
                frogclaw_core::mcp_client::discover_tools_stdio(command, &args, &env),
            )
            .await
            .map_err(|_| format!("Tool discovery timed out after {}s", timeout_secs))?
            .map_err(|e| e.to_string())?
        }
        "http" => {
            let endpoint = server
                .endpoint
                .as_deref()
                .ok_or_else(|| "HTTP server has no endpoint configured".to_string())?;
            tokio::time::timeout(
                timeout_duration,
                frogclaw_core::mcp_client::discover_tools_http(endpoint),
            )
            .await
            .map_err(|_| format!("Tool discovery timed out after {}s", timeout_secs))?
            .map_err(|e| e.to_string())?
        }
        "sse" => {
            let endpoint = server
                .endpoint
                .as_deref()
                .ok_or_else(|| "SSE server has no endpoint configured".to_string())?;
            tokio::time::timeout(
                timeout_duration,
                frogclaw_core::mcp_client::discover_tools_sse(endpoint),
            )
            .await
            .map_err(|_| format!("Tool discovery timed out after {}s", timeout_secs))?
            .map_err(|e| e.to_string())?
        }
        other => return Err(format!("Unsupported transport: {}", other)),
    };

    frogclaw_core::repo::mcp_server::save_tool_descriptors(&state.sea_db, &id, tools)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn list_tool_executions(
    state: State<'_, AppState>,
    conversation_id: String,
) -> Result<Vec<ToolExecution>, String> {
    frogclaw_core::repo::tool_execution::list_tool_executions(&state.sea_db, &conversation_id)
        .await
        .map_err(|e| e.to_string())
}
