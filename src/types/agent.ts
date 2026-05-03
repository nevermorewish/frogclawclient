export type AgentPermissionMode = 'default' | 'accept_edits' | 'full_access';
export type AgentRuntimeStatus = 'idle' | 'running' | 'waiting_approval' | 'completed' | 'error';
export type ApprovalStatus = 'pending' | 'approved' | 'denied';
export type AgentEngineKind = 'frog_agent' | 'claude_code' | 'codex_cli' | 'gemini_cli';

export interface AgentSession {
  id: string;
  conversation_id: string;
  cwd?: string;
  permission_mode: AgentPermissionMode;
  runtime_status: AgentRuntimeStatus;
  engine_kind?: AgentEngineKind;
  engineKind?: AgentEngineKind;
  engine_session_id?: string | null;
  engineSessionId?: string | null;
  engine_error?: string | null;
  engineError?: string | null;
  total_tokens: number;
  total_cost_usd: number;
}

export interface AgentEngineInfo {
  kind: AgentEngineKind;
  displayName: string;
  description: string;
  available: boolean;
  installed: boolean;
  version?: string | null;
  binaryPath?: string | null;
  status: 'available' | 'not_installed' | 'experimental' | string;
  message?: string | null;
  experimental: boolean;
}

// --- Event payload types (all tool-related events carry assistantMessageId anchor) ---

export interface ToolUseEvent {
  conversationId: string;
  assistantMessageId: string;
  toolUseId: string;
  toolName: string;
  input: Record<string, unknown>;
  executionId?: string;
}

export interface ToolStartEvent {
  conversationId: string;
  assistantMessageId: string;
  toolUseId: string;
  toolName: string;
  input: Record<string, unknown>;
}

export interface ToolResultEvent {
  conversationId: string;
  assistantMessageId: string;
  toolUseId: string;
  toolName: string;
  content: string;
  isError: boolean;
}

export interface PermissionRequestEvent {
  conversationId: string;
  assistantMessageId: string;
  toolUseId: string;
  toolName: string;
  input: Record<string, unknown>;
  riskLevel: 'read_only' | 'write' | 'execute';
}

export interface AskUserEvent {
  conversationId: string;
  assistantMessageId: string;
  askId: string;
  question: string;
  options?: string[];
}

export interface AgentDoneEvent {
  conversationId: string;
  assistantMessageId: string;
  text: string;
  usage?: { input_tokens: number; output_tokens: number };
  numTurns?: number;
  costUsd?: number;
}

export interface AgentErrorEvent {
  conversationId: string;
  assistantMessageId?: string;
  message: string;
}

export interface AgentCancelledEvent {
  conversationId: string;
  assistantMessageId?: string;
  reason: string;
}

export interface AgentStatusEvent {
  conversationId: string;
  message: string;
}

export interface AgentRateLimitEvent {
  conversationId: string;
  retryAfterMs: number;
  message: string;
}

export interface AgentStreamTextEvent {
  conversationId: string;
  assistantMessageId: string;
  text: string;
}

export interface AgentStreamThinkingEvent {
  conversationId: string;
  assistantMessageId: string;
  thinking: string;
}

// --- Frontend runtime state ---

export interface ToolCallState {
  toolUseId: string;
  toolName: string;
  input: Record<string, unknown>;
  assistantMessageId: string;
  executionStatus: 'queued' | 'running' | 'success' | 'failed' | 'cancelled';
  approvalStatus?: ApprovalStatus;
  output?: string;
  isError?: boolean;
}
