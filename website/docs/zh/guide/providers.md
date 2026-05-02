# 配置服务商

FrogClawClient 支持接入多种 AI 服务商，你可以同时配置多个服务商并在对话中自由切换。

## 支持的服务商

FrogClawClient 内置以下服务商类型，并支持任何兼容 OpenAI API 格式的自定义端点：

| 服务商 | 代表模型 | 说明 |
|-------|---------|------|
| **OpenAI** | GPT-4o、GPT-4、o3、o4-mini | 最广泛支持的 API 格式 |
| **Anthropic** | Claude 4 Sonnet/Opus、Claude 3.5 Sonnet | 原生 Claude API |
| **Google** | Gemini 2.5 Pro/Flash、Gemini 2.0 | Google AI Studio 或 Vertex |
| **DeepSeek** | DeepSeek-V3、DeepSeek-R1 | 高性价比推理模型 |
| **阿里云通义千问** | Qwen-Max、Qwen-Plus、Qwen-Turbo | 兼容 OpenAI 格式 |
| **智谱 GLM** | GLM-4、GLM-4-Flash | 国产大模型 |
| **xAI** | Grok-3、Grok-3-mini | xAI 出品 |
| **OpenAI 兼容** | 任意模型 | 适配所有兼容 OpenAI 格式的第三方服务 |

## 添加服务商

### 基本步骤

1. 进入 **设置 → 服务商**
2. 点击左下角的 **+** 按钮
3. 填写以下信息：
   - **名称** — 自定义名称，用于在界面中区分不同服务商
   - **类型** — 选择对应的服务商类型（如 OpenAI、Anthropic 等）
   - **图标** — 可选，为服务商选择一个显示图标
4. 点击确认创建

### 配置 API

创建服务商后，需要填写 API 连接信息：

#### API 密钥

填入从服务商处获取的 API Key。例如 OpenAI 的密钥格式为 `sk-...`。

::: tip 密钥安全
FrogClawClient 使用 AES-256 加密存储所有 API 密钥，密钥数据保存在本地 `~/.frogclaw/frogclaw.db` 中，不会上传到任何外部服务器。
:::

#### Base URL

API 的基础地址。各服务商的官方地址：

| 服务商 | 官方 Base URL |
|-------|--------------|
| OpenAI | `https://api.openai.com` |
| Anthropic | `https://api.anthropic.com` |
| Google | `https://generativelanguage.googleapis.com` |
| DeepSeek | `https://api.deepseek.com` |
| 阿里云通义 | `https://dashscope.aliyuncs.com/compatible-mode` |
| 智谱 GLM | `https://open.bigmodel.cn` |
| xAI | `https://api.x.ai` |

如果你使用第三方中转服务或自建代理，将 Base URL 替换为中转地址即可。

#### API 路径

API 请求的路径部分，默认为 `/v1/chat/completions`。一般情况下无需修改，除非服务商使用了非标准路径。

## 从网页链接导入服务商

服务商官网、中转服务后台、私有模型平台或本地网关页面，可以提供一个 **在 FrogClawClient 中打开** 的链接。用户点击后，浏览器会拉起 FrogClawClient 桌面端，FrogClawClient 会进入 **设置 → 服务商**，弹出确认框，并在用户确认后导入服务商配置。

### 用户使用流程

1. 安装并打开支持服务商链接导入的 FrogClawClient 版本。
2. 在浏览器中点击服务商页面提供的 **在 FrogClawClient 中打开** 链接。
3. 在 FrogClawClient 确认框中检查服务商名称、Base URL、服务商类型和 API Key 前缀。
4. 点击确认后，FrogClawClient 会按 **Base URL + 类型** 复用已有服务商；如果不存在则新建服务商，并在密钥不存在时追加 API Key。

FrogClawClient 不会自动验证 API Key，也不会自动同步模型列表。导入后你可以点击 **获取模型**，或手动添加模型 ID。

### 链接格式

```text
frogclawclient://providers?name=<name>&baseurl=<base-url>&apikey=<api-key>&type=<provider-type>
```

示例：

```text
frogclawclient://providers?name=OpenAI&baseurl=https%3A%2F%2Fapi.openai.com&apikey=sk-xxx&type=openai
```

### 参数说明

| 参数 | 必填 | 说明 |
|------|------|------|
| `name` | 是 | 服务商显示名称，例如 `OpenAI`、`我的中转服务` |
| `baseurl` | 是 | 服务商 Base URL，需要 URL 编码；仅允许 `http` / `https`，不允许 query 或 hash |
| `apikey` | 是 | 要保存到 FrogClawClient 的 API Key；确认框只展示前缀，不展示完整密钥 |
| `type` | 是 | 服务商类型；可选值：`openai`、`openai_responses`、`anthropic`、`gemini`、`custom` |

`baseurl` 支持 FrogClawClient 既有的强制后缀语义，例如 `https://example.com!`。通过链接导入时不会设置 `api_path`，FrogClawClient 会继续按所选服务商类型使用默认路径逻辑。

### 官网/后台页面如何配置链接

对所有动态值使用 `encodeURIComponent` 或 `URLSearchParams` 进行编码：

```html
<a id="open-frogclaw" href="#">在 FrogClawClient 中打开</a>

<script>
  const provider = {
    name: '我的中转服务',
    baseurl: 'https://api.example.com',
    apikey: 'sk-user-key',
    type: 'openai',
  };

  const params = new URLSearchParams({
    name: provider.name,
    baseurl: provider.baseurl,
    apikey: provider.apikey,
    type: provider.type,
  });

  document.getElementById('open-frogclaw').href = `frogclawclient://providers?${params.toString()}`;
</script>
```

如果你的服务支持在线生成 API Key，建议只在用户登录后、并且明确选择或创建密钥后再生成这个链接。

::: warning 安全提醒
URL 中携带 API Key 可能被浏览器历史、日志、浏览器扩展或统计脚本记录。不要把真实密钥写在公开页面、静态 HTML 或第三方跳转链接中；推荐在用户私有后台页面中按需生成，并让用户主动点击。
:::

::: tip 测试提示
`frogclawclient://` 是桌面应用注册到系统的自定义协议。仅运行官网或 Vite 开发服务不会注册协议；如果点击后没有拉起 FrogClawClient，请先安装或重新构建最新 FrogClawClient 桌面端。
:::

## 多密钥轮询

FrogClawClient 支持为同一个服务商配置多个 API 密钥，实现自动轮换：

### 添加多个密钥

在服务商配置页面的 API 密钥区域，可以添加多个密钥。所有密钥共享同一个 Base URL 和配置。

### 自动轮换机制

当配置了多个密钥时，FrogClawClient 会在每次请求时自动轮换使用不同的密钥。

### 限流分散

多密钥轮询的一个重要用途是分散 API 限流。每个密钥有独立的速率配额，使用多个密钥可以有效提高整体吞吐量，降低单个密钥触发限流的概率。

::: tip 适用场景
如果你有团队共享的多个 API 密钥，或者单个密钥的速率限制无法满足需求，多密钥轮询是一个简单高效的解决方案。
:::

## 模型管理

### 远程拉取模型列表

点击 **获取模型** 按钮，FrogClawClient 会调用服务商的模型列表 API，自动拉取当前可用的全部模型。拉取后的模型会显示在列表中供你选择。

### 手动添加模型

如果服务商的模型列表 API 不完整或你需要添加特定模型，可以手动输入模型 ID 进行添加。例如：

- `gpt-4o`
- `claude-sonnet-4-20250514`
- `gemini-2.5-pro`

### 模型参数

每个模型可以独立配置以下默认参数：

| 参数 | 说明 | 典型范围 |
|-----|------|---------|
| **温度 (Temperature)** | 控制输出的随机性，值越高越有创意 | 0 – 2 |
| **最大 Token (Max Tokens)** | 限制单次回复的最大长度 | 1 – 模型上限 |
| **Top-P** | 核采样概率阈值，与温度配合使用 | 0 – 1 |
| **频率惩罚 (Frequency Penalty)** | 降低已出现词汇的重复概率 | -2 – 2 |
| **存在惩罚 (Presence Penalty)** | 鼓励模型讨论新话题 | -2 – 2 |

::: info 参数建议
对于日常对话，保持默认参数即可。如果需要更有创意的输出，可以适当提高温度；需要精确回答时，降低温度到 0–0.3。
:::

## 自定义/本地端点

FrogClawClient 支持连接任何兼容 OpenAI API 格式的服务，以下是常见场景的配置示例。

### Ollama

[Ollama](https://ollama.ai/) 可以在本地运行开源大模型。配置方法：

1. 添加服务商，类型选择 **OpenAI**
2. Base URL 填写 `http://localhost:11434`
3. API Key 留空或填写任意值
4. 点击 **获取模型** 拉取本地已下载的模型

::: warning 注意
请确保 Ollama 服务已启动。部分 Ollama 版本可能需要设置 `OLLAMA_ORIGINS=*` 环境变量以允许跨域请求。
:::

### vLLM / TGI

如果你使用 vLLM 或 Text Generation Inference 部署了自己的模型：

1. 添加服务商，类型选择 **OpenAI**
2. Base URL 填写你的部署地址，如 `http://your-server:8000`
3. API Key 根据你的服务配置填写
4. 手动添加你部署的模型 ID

### API 中转服务

很多第三方提供 OpenAI 兼容的 API 中转服务，配置方法相同：

1. 添加服务商，类型选择 **OpenAI**
2. Base URL 填写中转服务提供的地址
3. API Key 填写中转服务提供的密钥
4. 获取或手动添加模型

## 默认模型设置

FrogClawClient 支持为不同用途设置默认模型，在 **设置 → 默认模型** 中配置：

### 全局默认助手模型

新建对话时自动使用的模型。建议选择你最常用的模型，例如 `gpt-4o` 或 `claude-sonnet-4-20250514`。

### 默认话题命名模型

用于自动为对话生成标题的模型。这个模型不需要很强大，选择一个响应快且便宜的模型即可，如 `gpt-4o-mini` 或 `gemini-2.0-flash`。

::: tip
默认模型设置是可选的。如果不设置，每次新建对话时需要手动选择模型。
:::
