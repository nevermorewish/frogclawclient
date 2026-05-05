# Agent 双后端架构

本文记录 FrogClaw 的 agent 后端演进方向：保留当前 `open-agent-sdk` 作为通用内嵌 agent 框架，同时接入 Codex 作为专用 coding agent 后端。该架构避免把通用业务 agent 和长时间软件工程 agent 混成一套实现。

## 背景

当前项目包含本地 Rust crate：

- `src-tauri/crates/open-agent-sdk`

该 crate 是轻量、可嵌入的 Rust agent SDK，适合在 Tauri 后端进程内直接运行 agent loop。它提供多 provider、工具调用、MCP、hooks、session persistence、permissions、subagents、tasks、compaction 等能力。

另一个候选后端是 Codex：

- 本地源码参考：`D:\frogclaw\codex`
- 主要实现：`codex-rs`

Codex 不是普通轻量 SDK，而是成熟的本地 coding agent 运行时。它面向真实代码仓库中的长时间工程任务，内置读写文件、shell 执行、diff/review、沙箱、审批、resume、compaction、TUI/app-server/SDK 等能力。

两者不建议直接互相替换。它们的定位不同：

- `open-agent-sdk`：通用 agent 框架，适合嵌入 FrogClaw 的业务功能。
- Codex：专用 coding agent runtime，适合代码库分析、修改、测试、review 和长时间工程任务。

## 目标

1. 保留 `open-agent-sdk` 的轻量嵌入优势。
2. 引入 Codex 的长时间 coding agent 能力。
3. 在 FrogClaw 内部建立统一的 agent 后端抽象。
4. 让 UI 和业务逻辑不直接依赖任一具体后端类型。
5. 支持按任务类型路由到不同 agent 后端。
6. 避免直接把 `codex-rs` 内部 crate 当成稳定业务 API 使用。

## 非目标

1. 不要求短期内删除 `open-agent-sdk`。
2. 不要求用 Codex 承担所有普通聊天和业务 agent 场景。
3. 不直接 fork 或重写 Codex core。
4. 不在第一阶段直接依赖大量 `codex-rs` 内部 crate。
5. 不把 Codex 的 workspace/sandbox/session 语义强行映射成普通业务 agent 语义。

## 总体架构

```text
FrogClaw UI
        |
Tauri backend
        |
Agent Orchestrator
        |
        +-- OpenAgentBackend
        |       基于 src-tauri/crates/open-agent-sdk
        |       用于普通聊天、文件问答、业务工具、轻量自动化
        |
        +-- CodexBackend
                基于 Codex SDK / app-server / CLI adapter
                用于代码库分析、修改、测试、review、长时间工程任务
```

核心原则：UI 只面对 FrogClaw 自己定义的 agent API，不直接面对 `open_agent_sdk::Agent` 或 Codex protocol 内部类型。

## 后端职责边界

### OpenAgentBackend

用于通用业务 agent 场景：

- 普通聊天。
- 文档总结和问答。
- FrogClaw 内部业务工具调用。
- 轻量文件操作。
- MCP 资源读取。
- 用户设置、任务、知识库、聊天记录等应用级功能。
- 需要多 provider 或 OpenAI-compatible endpoint 的场景，例如 OpenRouter、Ollama、LiteLLM。
- 需要直接在 Tauri 后端进程内运行、避免 CLI/subprocess 的场景。

默认不用于长时间自动修改代码库的任务。

### CodexBackend

用于 coding agent 场景：

- 分析代码仓库。
- 修改代码。
- 生成 patch。
- 运行测试、构建、lint、format。
- 根据命令输出继续修复。
- review 当前 diff。
- 长时间工程任务。
- 需要沙箱、审批、resume、compaction 的任务。
- 需要 Codex TUI/app-server/SDK 能力的任务。

默认不用于普通业务聊天、非代码工作流、轻量工具调用。

## 路由规则

Agent Orchestrator 根据任务类型选择后端。

优先走 `OpenAgentBackend` 的场景：

- 用户问题不涉及代码仓库修改。
- 用户请求是文档总结、聊天、问答、检索、业务工具调用。
- 用户明确指定使用通用 agent。
- 任务需要多 provider 或本地 OpenAI-compatible endpoint。

优先走 `CodexBackend` 的场景：

- 用户要求修 bug、改代码、重构、写测试。
- 用户要求运行项目测试、构建或调试命令。
- 用户要求 review diff 或生成 patch。
- 用户要求长时间完成一个开发任务。
- 用户明确指定使用 Codex。

需要用户确认或策略判断的场景：

- 任务既包含业务数据操作又包含代码修改。
- 任务会访问用户文件或执行 shell 命令。
- 任务可能需要网络、越界文件访问或高权限操作。

## 内部抽象建议

定义 FrogClaw 自己的后端接口，隐藏具体实现。

```rust
#[async_trait::async_trait]
pub trait AgentBackend: Send + Sync {
    async fn start_session(&self, request: StartAgentSessionRequest) -> AgentResult<AgentSessionInfo>;
    async fn send_message(&self, session_id: &str, message: AgentUserMessage) -> AgentResult<AgentEventStream>;
    async fn cancel(&self, session_id: &str) -> AgentResult<()>;
    async fn resume(&self, session_id: &str) -> AgentResult<AgentEventStream>;
    async fn close(&self, session_id: &str) -> AgentResult<()>;
}
```

事件类型也应由 FrogClaw 自己定义：

```rust
pub enum AgentEvent {
    SessionStarted { session_id: String },
    AssistantTextDelta { text: String },
    AssistantMessage { text: String },
    ToolStarted { id: String, name: String, input_preview: String },
    ToolOutput { id: String, output: String, is_error: bool },
    FileChanged { path: String },
    PermissionRequested { request: PermissionRequest },
    Progress { message: String },
    Completed { result: AgentRunResult },
    Cancelled,
    Error { message: String },
}
```

后端实现负责做类型映射：

- `OpenAgentBackend`：把 `SDKMessage` 映射为 `AgentEvent`。
- `CodexBackend`：把 Codex app-server/SDK/CLI events 映射为 `AgentEvent`。

## Codex 接入方式

CodexBackend 的接入方式按优先级选择。

### 1. Codex SDK / app-server

优先推荐。

优点：

- 更适合长期维护。
- 事件语义比解析 CLI 输出稳定。
- 更容易支持 thread、turn、resume、cancel、approval。
- 适合未来和 UI 做深度集成。

缺点：

- 初期接入成本高于 CLI 子进程。
- 需要认真设计协议适配层。

### 2. CLI 子进程

适合快速验证。

优点：

- 接入最快。
- 不需要直接依赖 `codex-rs` 内部 crate。
- 可以先跑通 coding worker 流程。

缺点：

- stdout/stderr 解析脆弱。
- 交互、审批、resume、事件映射成本较高。
- 对 UI 的实时状态表达不如 app-server 清晰。

### 3. MCP 互接

适合组合系统。

可能方向：

- 让主 agent 通过 MCP 调用 Codex coding worker。
- 让 Codex 通过 MCP 调用 FrogClaw 暴露的业务工具。

缺点：

- 需要定义明确的 MCP tool 契约。
- 不适合作为第一阶段的唯一集成方式。

### 4. 直接依赖 `codex-rs` 内部 crate

不推荐作为第一阶段方案。

风险：

- 内部 API 不一定稳定。
- crate 边界复杂。
- 和 Codex CLI/app-server/TUI 运行时耦合较深。
- 后续同步 upstream 成本高。

## Session 和存储策略

FrogClaw 仍应遵守项目存储策略：

- 应用状态存储在 `~/.frogclaw/`。
- 用户可见文件存储在 `~/Documents/frogclaw/`。
- 数据库中的用户文件路径使用 documents root 下的相对路径。

建议：

1. FrogClaw 自己维护统一的 agent session metadata。
2. `OpenAgentBackend` 可以继续使用自身 transcript/session，但需要把外部可见 session id 映射到 FrogClaw session id。
3. `CodexBackend` 可以维护 Codex thread/rollout/session id 到 FrogClaw session id 的映射。
4. 不把 Codex 的内部 transcript 路径直接暴露给 UI。
5. 长任务恢复时，以 FrogClaw session id 为入口，再查具体后端 session id。

建议数据库概念模型：

```text
agent_sessions
  id
  backend_kind          open_agent | codex
  backend_session_id
  title
  cwd
  model
  status
  created_at
  updated_at

agent_events
  id
  session_id
  sequence
  event_type
  payload_json
  created_at
```

## 权限和安全

OpenAgentBackend 和 CodexBackend 的权限模型不同，不能简单共用内部实现。

统一策略：

1. UI 层只展示 FrogClaw 的统一 permission request。
2. 后端适配层把各自权限事件映射为统一 permission request。
3. 代码修改、shell 执行、网络访问、越界文件访问必须可被策略层拦截。
4. 默认情况下，CodexBackend 只能在明确 workspace/cwd 下运行。
5. 不允许 agent 任意写入 FrogClaw 的 config home，除非该能力被明确授权。
6. 用户文件路径必须遵守 documents root 规则。

CodexBackend 需要特别注意：

- 不要默认使用 `danger-full-access`。
- 默认使用 workspace 级权限。
- 需要把审批请求呈现给用户或策略引擎。
- 需要保护项目敏感目录和用户数据目录。

## UI 行为

UI 不应暴露底层后端复杂度，但可以展示后端类型。

建议：

- 普通会话显示为 `Agent`。
- coding 会话显示为 `Codex` 或 `Coding Agent`。
- 工具调用、文件变更、命令执行、审批请求使用统一 UI 组件。
- CodexBackend 的 diff/review 结果应有专门视图。
- 长任务必须支持 cancel/resume。
- 后端切换应尽量发生在 session 创建时，不建议在同一 session 中频繁切换后端。

## 迁移计划

### 阶段 1：抽象层

1. 定义 `AgentBackend` trait。
2. 定义统一 `AgentEvent`。
3. 把当前 `open-agent-sdk` 调用包装为 `OpenAgentBackend`。
4. 保持现有功能行为不变。

### 阶段 2：CodexBackend 原型

1. 选择 Codex SDK/app-server 或 CLI 子进程作为第一版接入方式。
2. 支持最小闭环：
   - start session
   - send message
   - stream events
   - cancel
   - resume
3. 先支持代码分析和只读 review。
4. 再支持文件修改和命令执行。

### 阶段 3：路由和 UI

1. 引入任务路由策略。
2. 增加后端类型展示。
3. 增加 CodexBackend 的命令执行、文件变更、diff、approval UI。
4. 记录统一 agent events。

### 阶段 4：长任务能力

1. 完善 Codex session resume。
2. 支持 app 重启后恢复长任务。
3. 支持后台任务状态。
4. 支持失败重试和用户手动继续。

### 阶段 5：评估是否收敛

只有当 CodexBackend 覆盖所有需要的普通 agent 场景时，才评估是否删除 `open-agent-sdk`。在多数情况下，保留双后端更合理。

## 风险

主要风险：

- 两套后端事件模型不一致。
- Codex 权限和 FrogClaw 存储策略冲突。
- CLI 子进程方式可能不稳定。
- 直接依赖 `codex-rs` 内部 crate 会提高维护成本。
- 长任务 resume 会增加数据库和 UI 状态复杂度。
- Codex 适合 coding，不一定适合普通业务 agent。

缓解方式：

- 用 FrogClaw 自己的 `AgentBackend` 和 `AgentEvent` 隔离实现。
- 第一阶段只做 adapter，不重写现有业务逻辑。
- CodexBackend 先只读，再逐步开放写入和命令执行。
- 权限请求必须统一进入 FrogClaw 的审批/策略层。
- session id 和 backend session id 分开存储。

## 当前决策

采用双后端架构：

- `OpenAgentBackend` 继续承担通用 agent 场景。
- `CodexBackend` 作为 coding agent 专用后端引入。
- 短期不删除 `src-tauri/crates/open-agent-sdk`。
- 短期不直接用 `codex-rs` 替换 `open-agent-sdk`。
- 优先通过 Codex SDK/app-server 或 CLI adapter 集成 Codex。

该决策可以在 CodexBackend 原型完成、真实任务验证稳定后重新评估。
