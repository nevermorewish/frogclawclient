# Configure Providers

FrogClawClient connects to any number of AI providers simultaneously. Each provider has its own API keys, model list, and parameter defaults.

## Supported Providers

FrogClawClient includes first-class support for the following providers. Any service that exposes an OpenAI-compatible API also works out of the box.

| Provider | Example Models |
|----------|---------------|
| **OpenAI** | GPT-4o, GPT-4, o3, o4-mini |
| **Anthropic** | Claude 4 Sonnet, Claude 4 Opus, Claude 3.5 Sonnet |
| **Google** | Gemini 2.5 Pro, Gemini 2.5 Flash, Gemini 2.0 |
| **DeepSeek** | DeepSeek V3, DeepSeek R1 |
| **Alibaba Cloud** | Qwen series |
| **Zhipu AI** | GLM series |
| **xAI** | Grok series |
| **Any OpenAI-compatible API** | Ollama, vLLM, LiteLLM, third-party relays, etc. |

---

## Adding a Provider

1. Go to **Settings → Providers**.
2. Click the **+** button at the bottom left.
3. Fill in the provider details:

| Field | Description |
|-------|-------------|
| **Name** | A display name for the sidebar (e.g. *OpenAI*) |
| **Type** | Provider type — determines the default base URL and API behavior |
| **Icon** | Optional icon for visual identification |
| **API Key** | The secret key from your provider's dashboard |
| **Base URL** | API endpoint (pre-filled for built-in types, changeable for relays) |
| **API Path** | Request path — defaults to `/v1/chat/completions` |

::: tip
For third-party relay services, keep the type set to **OpenAI** (or the matching upstream type) and change the **Base URL** to the relay's endpoint.
:::

---

## Importing from a Website Link

Provider websites can offer an **Open in FrogClawClient** link that opens the FrogClawClient desktop app and pre-fills the provider settings. FrogClawClient will switch to **Settings → Providers**, show a confirmation dialog, and import the provider only after the user confirms.

This is useful for API vendors, relay services, private model platforms, and local gateway dashboards that want to help users configure FrogClawClient without copying every field manually.

### User Flow

1. Install and open a version of FrogClawClient that supports provider links.
2. Click the provider's **Open in FrogClawClient** link in your browser.
3. Confirm the provider name, Base URL, provider type, and API key prefix in FrogClawClient.
4. FrogClawClient creates a new provider or reuses an existing provider with the same **Base URL + type**, then adds the API key if it is not already present.

FrogClawClient does not validate the key or fetch models automatically. After importing, click **Fetch Models** or add models manually.

### Link Format

```text
frogclawclient://providers?name=<name>&baseurl=<base-url>&apikey=<api-key>&type=<provider-type>
```

Example:

```text
frogclawclient://providers?name=OpenAI&baseurl=https%3A%2F%2Fapi.openai.com&apikey=sk-xxx&type=openai
```

### Parameters

| Parameter | Required | Description |
|-----------|----------|-------------|
| `name` | Yes | Display name shown in FrogClawClient, for example `OpenAI` or `My Relay` |
| `baseurl` | Yes | Provider Base URL, URL-encoded. Only `http` and `https` are accepted. Query strings and fragments are rejected. |
| `apikey` | Yes | API key to save into FrogClawClient. FrogClawClient shows only a prefix in the confirmation dialog. |
| `type` | Yes | Provider type. Allowed values: `openai`, `openai_responses`, `anthropic`, `gemini`, `custom`. |

`baseurl` can use FrogClawClient's existing force suffix, for example `https://example.com!`. FrogClawClient stores `api_path` as empty for imported providers and continues to use the default path for the selected provider type.

### Website Configuration

Use `encodeURIComponent` for every dynamic value:

```html
<a id="open-frogclaw" href="#">Open in FrogClawClient</a>

<script>
  const provider = {
    name: 'My Relay',
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

If your service lets users generate API keys, create the link only after the user is signed in and has explicitly selected or created a key.

::: warning Security
An API key in a URL may be visible to browser history, logs, extensions, or analytics tools. Do not place provider import links with real keys on public pages, in static HTML, or in third-party tracking redirects. Prefer generating the link on a private account page after user confirmation.
:::

::: tip Testing
Custom URI schemes are registered by the installed desktop app. If `frogclawclient://...` does not open FrogClawClient, install or rebuild the latest FrogClawClient app first; running only the website or Vite dev server does not register the system scheme.
:::

---

## Multi-Key Rotation

FrogClawClient supports multiple API keys per provider for load distribution and rate-limit avoidance.

### Adding Keys

In the provider detail panel, click **Add Key** to add additional API keys. Each key shows its prefix, creation date, and last-used timestamp.

### How Rotation Works

FrogClawClient rotates through enabled keys automatically using a round-robin index. When a request completes, the rotation index advances to the next key. If a key is disabled or fails validation, it is skipped.

### Validating Keys

Click the **validate** button next to a key to verify it against the provider's API. The validation result and any error message are recorded for reference.

---

## Model Management

### Fetching Models

Click **Fetch Models** in the provider detail panel to pull the full list of available models from the provider's API. The discovered models are added to your local list automatically.

### Adding Models Manually

If a model is not returned by the API (e.g. a fine-tuned model or a new release), you can add it by typing the model ID directly.

### Per-Model Parameters

Each model can have its own default parameter overrides. Open the model's settings to configure:

| Parameter | Description |
|-----------|-------------|
| **Temperature** | Controls randomness (0 = deterministic, higher = more creative) |
| **Max Tokens** | Maximum number of tokens in the response |
| **Top P** | Nucleus sampling threshold |
| **Frequency Penalty** | Reduces repetition of token sequences |
| **Presence Penalty** | Encourages the model to introduce new topics |

### Model Capabilities

FrogClawClient tracks capabilities per model — such as **Vision**, **Function Calling**, and **Reasoning** — and displays them as tags. These capabilities affect which features are available during a conversation.

---

## Custom & Local Endpoints

FrogClawClient works with any endpoint that implements the OpenAI chat completions API.

### Ollama (Local Models)

1. Install and start [Ollama](https://ollama.com/).
2. In FrogClawClient, create a new provider with type **OpenAI**.
3. Set the **Base URL** to `http://localhost:11434`.
4. Click **Fetch Models** to discover the models you have pulled locally.

### vLLM / TGI

Point the **Base URL** to the address of your inference server (e.g. `http://localhost:8000`) and fetch or add models as usual.

### API Relay Services

For relay or aggregator services (e.g. OpenRouter, one-api), set the type to **OpenAI**, enter the relay's base URL, and provide the relay's API key.

---

## Default Model Settings

### Default Assistant Model

Go to **Settings → Default Model** to choose the provider and model that new conversations use by default. You can always override the model on a per-conversation basis from the model selector.

### Topic Naming Model

FrogClawClient can automatically generate a title for each conversation. In the default model settings, you can assign a separate, lightweight model for topic naming to save cost and latency. Configure a custom prompt and context window size for title generation.

---

## Next Steps

- [MCP Servers](./mcp) — connect external tools to extend AI capabilities
- [API Gateway](./gateway) — expose your providers as a local API server
