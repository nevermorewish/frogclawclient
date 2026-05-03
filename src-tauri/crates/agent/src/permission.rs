use serde::{Deserialize, Serialize};

/// Tool risk classification
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RiskLevel {
    ReadOnly,
    Write,
    Execute,
}

/// Permission mode for the agent session
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PermissionMode {
    Default,
    AcceptEdits,
    FullAccess,
}

impl PermissionMode {
    pub fn from_str(s: &str) -> Self {
        match s {
            "accept_edits" => Self::AcceptEdits,
            "full_access" => Self::FullAccess,
            _ => Self::Default,
        }
    }
}

/// What the permission system decides
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionAction {
    AutoAllow,
    RequireApproval,
    HardDeny,
}

/// Runtime-agnostic permission decision used by FrogClaw.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PermissionDecision {
    Allow,
    Deny(String),
}

/// Classify a tool's risk level based on its name
pub fn classify_tool_risk(tool_name: &str) -> RiskLevel {
    let name_lower = tool_name.to_lowercase();

    // Execute-level tools
    if matches!(
        name_lower.as_str(),
        "bash" | "shell" | "run_command" | "execute"
    ) || name_lower.contains("exec")
        || name_lower.contains("run")
        || name_lower.contains("bash")
        || name_lower.contains("shell")
    {
        return RiskLevel::Execute;
    }

    // Write-level tools
    if matches!(
        name_lower.as_str(),
        "write"
            | "edit"
            | "create"
            | "delete"
            | "rename"
            | "patch"
            | "write_file"
            | "edit_file"
            | "create_file"
            | "delete_file"
            | "move"
            | "mkdir"
            | "remove"
    ) || name_lower.contains("write")
        || name_lower.contains("edit")
        || name_lower.contains("create")
        || name_lower.contains("delete")
        || name_lower.contains("patch")
    {
        return RiskLevel::Write;
    }

    // Everything else is read-only
    RiskLevel::ReadOnly
}

/// Decision matrix: given permission mode, risk level, and whether the tool
/// is in the "always allowed" set, return the action to take.
pub fn decide_permission(
    mode: PermissionMode,
    risk: RiskLevel,
    is_always_allowed: bool,
) -> PermissionAction {
    // If tool was previously approved with "always allow", auto-allow
    if is_always_allowed {
        return PermissionAction::AutoAllow;
    }

    match (mode, risk) {
        // Default mode: only read is auto-allowed
        (PermissionMode::Default, RiskLevel::ReadOnly) => PermissionAction::AutoAllow,
        (PermissionMode::Default, RiskLevel::Write) => PermissionAction::RequireApproval,
        (PermissionMode::Default, RiskLevel::Execute) => PermissionAction::RequireApproval,

        // Accept edits: read + write auto-allowed
        (PermissionMode::AcceptEdits, RiskLevel::ReadOnly) => PermissionAction::AutoAllow,
        (PermissionMode::AcceptEdits, RiskLevel::Write) => PermissionAction::AutoAllow,
        (PermissionMode::AcceptEdits, RiskLevel::Execute) => PermissionAction::RequireApproval,

        // Full access: everything auto-allowed
        (PermissionMode::FullAccess, _) => PermissionAction::AutoAllow,
    }
}
