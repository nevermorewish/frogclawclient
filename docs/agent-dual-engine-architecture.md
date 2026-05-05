# Agent 双引擎架构设计

本文设计 FrogClaw 的 Agent 引擎架构：保留当前内置 `open-agent-sdk` Agent，同时新增 `Claude Code` 引擎，并预留 `Codex CLI`、`Gemini CLI` 等后续引擎扩展位。首期以双引擎落地，但抽象层必须按多引擎设计，避免后续再拆一次。

## 目标

1. Agent 模式下支持切换执行引擎：
   - `Frog Agent`：当前内置 `open-agent-sdk`。
   - `Claude Code`：通过 Claude Code CLI / Claude Code SDK 能力执行。
   - `Codex CLI`：预留，通过 Codex CLI / app-server adapter 执行。
   - `Gemini CLI`：预留，通过 Gemini CLI adapter 执行。
2. UI 不直接依赖具体引擎类型，只消费统一 Agent 事件。
3. 每个会话记录自己使用的引擎，避免同一会话中上下文语义混乱。
4. 工作目录继续绑定项目/文件夹，不再使用分类概念。
5. 权限、日志、取消、状态栏、工具调用卡片尽量复用现有 Agent UI。

## 参考 CodePilot 架构

CodePilot 的核心思路是 runtime registry：

```text
chat route / conversation engine
        |
        v
resolveRuntime()
        |
        +-- native runtime
        |     内置 agent loop，直接调用 provider API
        |
        +-- claude-code-sdk runtime
              Claude Code CLI / SDK，复用 Claude Code 的工具和权限能力
```

关键点：

- 所有 runtime 实现同一个 `AgentRuntime` 接口。
- 发送入口只调用 `runtime.stream(...)`。
- 前端只消费统一事件流，不知道底层是 native 还是 Claude Code。
- 设置里有全局默认引擎；聊天页右下角有当前引擎提示/切换。

FrogClaw 可以采用同样思想，但后端是 Tauri Rust，建议使用 Rust trait 和 Tauri event。不要把字段命名限制为 `claude` 或 `sdk`，统一使用 `engine`。

## 总体结构

```text
Chat UI
  InputArea
    mode: chat | agent
    agent_engine: frog_agent | claude_code
        |
        v
Tauri command: agent_query
        |
        v
AgentEngineRegistry / AgentEngineResolver
        |
        +-- FrogAgentEngine
        |     当前 open-agent-sdk 实现
        |
        +-- ClaudeCodeEngine
        |     Claude Code CLI/SDK adapter
        |
        +-- CodexCliEngine
        |     Codex CLI/app-server adapter
        |
        +-- GeminiCliEngine
              Gemini CLI adapter
```

## 引擎类型

建议定义稳定枚举：

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentEngineKind {
    FrogAgent,
    ClaudeCode,
    CodexCli,
    GeminiCli,
}
```

前端类型：

```ts
export type AgentEngineKind = 'frog_agent' | 'claude_code' | 'codex_cli' | 'gemini_cli';
```

显示文案：

```text
frog_agent  -> Frog Agent
claude_code -> Claude Code
codex_cli   -> Codex CLI
gemini_cli  -> Gemini CLI
```

## 后端 Trait

新增 `src-tauri/crates/agent/src/engine.rs` 或 `src-tauri/src/agent_engine/`：

```rust
#[async_trait::async_trait]
pub trait AgentEngine: Send + Sync {
    fn kind(&self) -> AgentEngineKind;
    fn display_name(&self) -> &'static str;

    async fn is_available(&self) -> AgentResult<EngineAvailability>;

    async fn query(&self, ctx: AgentRunContext) -> AgentResult<()>;

    async fn cancel(&self, conversation_id: &str) -> AgentResult<()>;

    async fn approve(&self, request_id: &str, decision: AgentApprovalDecision) -> AgentResult<()>;

    async fn respond_ask(&self, ask_id: &str, answer: String) -> AgentResult<()>;
}
```

`AgentRunContext` 放所有引擎都需要的字段：

```rust
pub struct AgentRunContext {
    pub app: tauri::AppHandle,
    pub db: sea_orm::DatabaseConnection,
    pub conversation_id: String,
    pub session_id: String,
    pub prompt: String,
    pub provider_id: String,
    pub model_id: String,
    pub cwd: PathBuf,
    pub permission_mode: String,
    pub attachments: Vec<AgentAttachment>,
}
```

## 统一事件

继续复用现有 Tauri 事件名，避免大改前端：

```text
agent-message-id
agent-stream-text
agent-stream-thinking
agent-tool-start
agent-tool-use
agent-tool-result
agent-permission-request
agent-ask-user
agent-status
agent-rate-limit
agent-error
agent-done
```

新增事件字段 `engine`，用于 UI 展示和调试：

```json
{
  "conversationId": "conv_xxx",
  "engine": "claude_code",
  "message": "Running command..."
}
```

前端老逻辑可以忽略该字段。

## 数据库变更

当前 `agent_sessions` 已有：

```text
id
conversation_id
cwd
permission_mode
runtime_status
sdk_context_json
sdk_context_backup_json
total_tokens
total_cost_usd
created_at
updated_at
```

建议新增字段：

```text
engine_kind TEXT NOT NULL DEFAULT 'frog_agent'
engine_session_id TEXT NULL
engine_context_json TEXT NULL
engine_context_backup_json TEXT NULL
engine_status TEXT NOT NULL DEFAULT 'idle'
engine_error TEXT NULL
```

字段含义：

- `engine_kind`：当前会话选择的 Agent 引擎。
- `engine_session_id`：外部引擎会话 ID，例如 Claude Code session ID。
- `engine_context_json`：引擎自己的上下文或 resume 数据。
- `engine_context_backup_json`：上下文清理前备份。
- `engine_status`：`idle | running | waiting_permission | error`。
- `engine_error`：最近一次错误。

兼容策略：

- 旧 `sdk_context_json` 暂时保留，映射为 `frog_agent` 的上下文。
- 新代码优先读 `engine_context_json`。
- 迁移后不要马上删除旧字段，避免破坏已有会话。

## 引擎切换规则

推荐会话级切换，不推荐单轮请求级随意切换。

规则：

1. 新建 Agent 会话时使用全局默认引擎。
2. 用户可以在右下角切换当前会话引擎。
3. 如果当前会话已有消息，切换时提示：
   - “切换 Agent 引擎会使用新的运行上下文，旧引擎上下文不会继续。”
4. 切换后：
   - `engine_kind` 更新为新引擎。
   - 不删除消息。
   - 不强行复用旧引擎上下文。
   - `engine_session_id` 和 `engine_context_json` 按新引擎清空或重新初始化。
5. 正在运行时不允许切换。

## UI 设计

位置：对话输入框下方右侧，和当前权限模式、上下文 token 指示并排。

```text
[问答] [Agent]       [目录]             [Agent 引擎: Frog Agent v] [权限] [上下文]
```

当 `currentMode !== 'agent'` 时不显示 Agent 引擎切换。

下拉项：

```text
Frog Agent
  内置 Agent 引擎，使用当前模型服务商

Claude Code
  Claude Code 引擎，需要本机安装并登录 Claude Code CLI

Codex CLI
  Codex 引擎，需要本机安装并登录 Codex CLI

Gemini CLI
  Gemini 引擎，需要本机安装并登录 Gemini CLI
```

状态：

- `Frog Agent` 永远可选，只要当前 provider/model 可用于 Agent。
- `Claude Code` 需要检测 CLI：
  - 可用：正常显示版本号。
  - 不可用：显示“未安装”，下拉里提供“安装/打开设置”。
- `Codex CLI`、`Gemini CLI` 同样走 engine status 检测，首期可以先显示为“未启用/实验性”。

建议新增前端 store 字段：

```ts
interface AgentSession {
  engine_kind: AgentEngineKind;
  engine_session_id?: string | null;
  cwd?: string | null;
  permission_mode: string;
  runtime_status: string;
}
```

新增命令：

```text
agent_list_engines
agent_update_session_engine
agent_get_engine_status
```

## Claude Code 引擎接入方式

第一阶段不要把 Claude Code 直接嵌到主进程复杂运行时里，建议走子进程 adapter：

```text
ClaudeCodeEngine
  find claude binary
  spawn claude command
  read stdout/stderr/jsonl
  map events to AgentEvent
  emit Tauri events
```

检测路径参考 CodePilot：

- Windows:
  - `%USERPROFILE%\.local\bin\claude.exe`
  - `%USERPROFILE%\.claude\bin\claude.exe`
  - `%APPDATA%\npm\claude.cmd`
  - `%LOCALAPPDATA%\npm\claude.cmd`
- macOS/Linux:
  - `~/.local/bin/claude`
  - `~/.claude/bin/claude`
  - `/opt/homebrew/bin/claude`
  - `/usr/local/bin/claude`
  - `which claude`

第一版能力边界：

- 支持发送 prompt。
- 支持 cwd。
- 支持 cancel。
- 支持 stdout/stderr 日志。
- 支持最终文本回写。
- 权限先按 Claude Code 自己的交互/CLI 能力处理，后续再映射到 FrogClaw 的 permission card。

第二版再支持：

- Claude Code session resume。
- JSON event stream。
- tool call 结构化映射。
- permission request 结构化映射。
- diff/file changed 事件。

## Codex CLI 与 Gemini CLI 扩展

不要为每个 CLI 单独创造一套前后端事件。Codex CLI、Gemini CLI 应该和 Claude Code 一样实现 `AgentEngine`：

```text
CodexCliEngine
  detect binary
  resolve auth/status
  spawn process or connect app-server
  parse stdout/stderr/json events
  emit unified Agent events

GeminiCliEngine
  detect binary
  resolve auth/status
  spawn process
  parse stdout/stderr/json events
  emit unified Agent events
```

建议统一 CLI adapter 基类：

```rust
pub trait CliAgentAdapter: Send + Sync {
    fn binary_name(&self) -> &'static str;
    fn candidate_paths(&self) -> Vec<PathBuf>;
    fn build_args(&self, ctx: &AgentRunContext) -> Vec<String>;
    fn parse_event(&self, line: &str) -> Option<AgentEngineEvent>;
}
```

然后：

```text
ClaudeCodeEngine -> ClaudeCodeCliAdapter
CodexCliEngine   -> CodexCliAdapter
GeminiCliEngine  -> GeminiCliAdapter
```

这样不同 CLI 只处理自己的二进制检测、参数构造、事件解析；权限、消息持久化、Tauri 事件、日志、取消、运行状态由共享 engine runner 处理。

### Codex CLI 优先级

Codex 不建议第一阶段直接引入 `codex-core`。优先级：

1. `codex app-server` / protocol adapter。
2. `codex` CLI subprocess adapter。
3. 稳定 adapter crate。
4. 最后才考虑直接嵌入 Rust core。

### Gemini CLI 优先级

Gemini CLI 第一阶段只做子进程 adapter：

1. 检测 `gemini` 二进制。
2. 验证版本和登录状态。
3. 指定 `cwd`。
4. 流式读取输出。
5. 最小映射 `status/text/error/done`。

等 CLI 有稳定 JSON event output 后，再映射 tool call、permission、file changed。

## 日志

日志页新增：

```text
Frog Agent 日志
Claude Code 日志
Codex CLI 日志
Gemini CLI 日志
Sidecar 日志
安装日志
```

不要恢复 OpenClaw 日志。

CLI 引擎日志来源：

- CLI 检测日志。
- spawn 参数摘要，敏感信息脱敏。
- stdout/stderr ring buffer。
- 退出码和错误。

## 权限模型

`FrogAgentEngine` 继续使用当前 `permission_mode`：

```text
default
accept_edits
full_access
```

CLI 引擎第一阶段建议保守：

- 默认只能在当前 `cwd` 下运行。
- 不允许自动使用 full access。
- 如果 CLI 提供 approval 模式，就映射到现有 permission card。
- 如果无法结构化拦截权限，则 UI 要显示“当前引擎权限由 CLI 控制”。

## 取消与并发

当前代码用 `RUNNING_AGENTS` 和 cancel token 限制同一会话并发。双引擎后改成：

```rust
RUNNING_AGENT_RUNS: HashMap<String, RunningAgentRun>

struct RunningAgentRun {
    run_id: String,
    engine_kind: AgentEngineKind,
}
```

取消时：

```text
agent_cancel(conversation_id)
  -> 查 session.engine_kind
  -> registry.get(engine_kind).cancel(conversation_id)
  -> DB status idle
```

CLI 引擎取消就是 kill child process / send interrupt。app-server 类引擎优先发 interrupt/cancel request。

## 推荐实施顺序

### 阶段 1：抽象层

1. 新增 `AgentEngineKind`。
2. 新增 `AgentEngine` trait。
3. 把当前 `agent_query` 的 open-agent-sdk 逻辑搬进 `FrogAgentEngine`。
4. 保持前端行为不变。

### 阶段 2：会话字段和 UI

1. `agent_sessions` 增加 `engine_kind`。
2. `agent_get_session` 返回 `engine_kind`。
3. `agent_update_session` 支持更新 engine。
4. `InputArea` 右下角 Agent 模式下显示引擎下拉框。

### 阶段 3：Claude Code 原型

1. 实现 Claude CLI 检测。
2. 新增 `agent_list_engines` 返回可用状态。
3. 实现 `ClaudeCodeEngine` 子进程版。
4. 映射最小事件：status、text、error、done。

### 阶段 4：结构化事件

1. 支持 Claude Code JSON 输出或 SDK event stream。
2. 映射 tool start/result。
3. 映射 permission request。
4. 支持 resume。

### 阶段 5：统一设置

1. 设置页新增“默认 Agent 引擎”。
2. 新建 Agent 会话时使用默认引擎。
3. 会话右下角可以覆盖默认引擎。

## 最终建议

第一版不要把 Claude Code 逻辑揉进当前 `agent_query`。当前 `agent_query` 已经同时承担 provider、权限、消息持久化、tool 映射、标题生成、上下文备份等职责，再直接塞 Claude Code 会变得不可维护。

正确切法是先做 `AgentEngine` 抽象：

```text
agent_query
  只负责读取会话、创建用户消息、解析 engine_kind、调 registry

FrogAgentEngine
  承接现有 open-agent-sdk 大逻辑

ClaudeCodeEngine
  独立实现 Claude Code adapter
```

这样 UI 可以像 CodePilot 一样切换“Agent 引擎”，而后端不会把两个运行时混在同一段流程里。
