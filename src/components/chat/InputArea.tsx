import React, { useState, useRef, useCallback, useEffect, useMemo } from 'react';
import { Button, Tooltip, App, theme, Dropdown, Tag, Popover } from 'antd';
import type { MenuProps } from 'antd';
import { Paperclip, Trash2, Mic, Eraser, Scissors, Globe, Atom, Plug, ArrowUp, Square, Check, Zap, ZapOff, Shrink, Upload, GripHorizontal, CircleOff, SignalLow, SignalMedium, SignalHigh, Signal, Shield, ShieldCheck, ShieldAlert, FolderOpen, ExternalLink, ChevronDown, Terminal } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { useAgentStore, useConversationStore, useProviderStore, useSettingsStore, useSearchStore, useMcpStore, useMemoryStore } from '@/stores';
import { useUIStore } from '@/stores/uiStore';
import { findModelByIds, supportsReasoning, modelHasCapability } from '@/lib/modelCapabilities';
import {
  coerceReasoningOptionKey,
  legacyThinkingBudgetToOptionKey,
  resolveReasoningProfile,
} from '@/lib/reasoningProfile';
import { estimateMessageTokens, estimateTokens } from '@/lib/tokenEstimator';
import { getShortcutBinding, formatShortcutForDisplay, matchesShortcutEvent } from '@/lib/shortcuts';
import type { ShortcutAction } from '@/lib/shortcuts';
import { VoiceCall } from './VoiceCall';
import { ModelSelector } from './ModelSelector';
import { SearchProviderTypeIcon, PROVIDER_TYPE_LABELS } from '@/components/shared/SearchProviderIcon';
import type { AttachmentInput, ProviderType, RealtimeConfig } from '@/types';
import type { AgentEngineInfo, AgentEngineKind } from '@/types/agent';
import { invoke } from '@/lib/invoke';
import { open } from '@tauri-apps/plugin-dialog';

async function fileToAttachmentInput(file: File): Promise<AttachmentInput> {
  return new Promise((resolve) => {
    const reader = new FileReader();
    reader.onload = () => {
      const base64 = (reader.result as string).split(',')[1] || '';
      resolve({
        file_name: file.name,
        file_type: file.type || 'application/octet-stream',
        file_size: file.size,
        data: base64,
      });
    };
    reader.readAsDataURL(file);
  });
}

// In-memory draft cache: persists input text per-conversation across component unmounts
const _draftCache = new Map<string, string>();
const DEFAULT_AGENT_ENGINE_KEY = 'frogclaw:default-agent-engine';
const SUPPORTED_AGENT_ENGINE_KINDS: AgentEngineKind[] = ['codex_app_server', 'frog_agent', 'claude_code', 'codex_cli', 'gemini_cli'];
const DEFAULT_AGENT_PERMISSION_MODE = 'full_access';

function readDefaultAgentEngine(): AgentEngineKind {
  try {
    const stored = localStorage.getItem(DEFAULT_AGENT_ENGINE_KEY) as AgentEngineKind | null;
    return stored && SUPPORTED_AGENT_ENGINE_KINDS.includes(stored) ? stored : 'codex_app_server';
  } catch {
    return 'codex_app_server';
  }
}

function writeDefaultAgentEngine(engine: AgentEngineKind) {
  try {
    localStorage.setItem(DEFAULT_AGENT_ENGINE_KEY, engine);
  } catch {
    // localStorage can be unavailable in tests or restricted webviews.
  }
}

function resolveNativeCliEngine(providerType?: ProviderType, modelId?: string | null): AgentEngineKind {
  const model = (modelId || '').toLowerCase();
  if (providerType === 'gemini' || model.includes('gemini')) return 'gemini_cli';
  if (
    providerType === 'openai'
    || providerType === 'openai_responses'
    || model.includes('gpt')
    || model.includes('codex')
    || /^o\d/.test(model)
  ) {
    return 'codex_cli';
  }
  return 'claude_code';
}

function formatAgentEngineLabel(engine: AgentEngineKind): string {
  switch (engine) {
    case 'claude_code': return 'Claude Code';
    case 'codex_cli': return 'Codex CLI';
    case 'gemini_cli': return 'Gemini CLI';
    default: return 'AIAgent';
  }
}

export function InputArea() {
  const { t } = useTranslation();
  const { token } = theme.useToken();
  const [value, setValue] = useState(() => {
    const convId = useConversationStore.getState().activeConversationId;
    return convId ? _draftCache.get(convId) || '' : '';
  });
  const [attachedFiles, setAttachedFiles] = useState<File[]>([]);
  const [voiceCallVisible, setVoiceCallVisible] = useState(false);
  const [searchDropdownOpen, setSearchDropdownOpen] = useState(false);
  const fileInputRef = useRef<HTMLInputElement>(null);
  const textareaRef = useRef<HTMLTextAreaElement>(null);
  const valueRef = useRef(value);
  valueRef.current = value;
  const prevConvIdRef = useRef<string | null>(
    useConversationStore.getState().activeConversationId ?? null
  );

  // Drag-to-resize state: userMinHeight controls the minimum visible height of the textarea
  const INITIAL_MIN_HEIGHT = 44;
  const ABSOLUTE_MAX_HEIGHT = 600;
  const [userMinHeight, setUserMinHeight] = useState(INITIAL_MIN_HEIGHT);
  const userMinHeightRef = useRef(userMinHeight);
  userMinHeightRef.current = userMinHeight;
  const dragStateRef = useRef<{ startY: number; startH: number } | null>(null);
  const hasUserResizedRef = useRef(false);
  const containerRef = useRef<HTMLDivElement>(null);

  // Multi-model companion state
  const [companionModels, setCompanionModels] = useState<Array<{ providerId: string; modelId: string }>>([]);

  const { message: messageApi, modal } = App.useApp();
  const streaming = useConversationStore((s) => s.streaming);
  const compressing = useConversationStore((s) => s.compressing);
  const cancelCurrentStream = useConversationStore((s) => s.cancelCurrentStream);
  const activeConversationId = useConversationStore((s) => s.activeConversationId);
  const sendAgentMessage = useConversationStore((s) => s.sendAgentMessage);
  const createConversation = useConversationStore((s) => s.createConversation);
  const messages = useConversationStore((s) => s.messages);
  const totalActiveCount = useConversationStore((s) => s.totalActiveCount);
  const hasOlderMessages = useConversationStore((s) => s.hasOlderMessages);
  const contextCount = useMemo(() => {
    const activeMessages = messages.filter((m) => m.is_active !== false && !m.content.startsWith('%%ERROR%%'));
    const lastMarkerIdx = activeMessages.reduce((maxIdx, m, i) => {
      if (m.content === '<!-- context-clear -->' || m.content === '<!-- context-compressed -->') return i;
      return maxIdx;
    }, -1);
    if (lastMarkerIdx !== -1) {
      return activeMessages.slice(lastMarkerIdx + 1).length;
    }
    if (hasOlderMessages && totalActiveCount > 0) {
      return totalActiveCount;
    }
    return activeMessages.length;
  }, [messages, hasOlderMessages, totalActiveCount]);

  const conversations = useConversationStore((s) => s.conversations);
  const providers = useProviderStore((s) => s.providers);
  const settings = useSettingsStore((s) => s.settings);

  const shortcutHint = useCallback((label: string, action: ShortcutAction) => {
    if (!settings) return label;
    const binding = getShortcutBinding(settings, action);
    return `${label} (${formatShortcutForDisplay(binding)})`;
  }, [settings]);

  // Search state
  const searchEnabled = useConversationStore((s) => s.searchEnabled);
  const searchProviderId = useConversationStore((s) => s.searchProviderId);
  const setSearchEnabled = useConversationStore((s) => s.setSearchEnabled);
  const setSearchProviderId = useConversationStore((s) => s.setSearchProviderId);
  const searchProviders = useSearchStore((s) => s.providers);
  const loadSearchProviders = useSearchStore((s) => s.loadProviders);

  // MCP state
  const mcpServers = useMcpStore((s) => s.servers);
  const loadMcpServers = useMcpStore((s) => s.loadServers);
  const enabledMcpServerIds = useConversationStore((s) => s.enabledMcpServerIds);
  const toggleMcpServer = useConversationStore((s) => s.toggleMcpServer);

  // Thinking state
  const thinkingBudget = useConversationStore((s) => s.thinkingBudget);
  const setThinkingBudget = useConversationStore((s) => s.setThinkingBudget);
  const thinkingLevel = useConversationStore((s) => s.thinkingLevel);
  const setThinkingLevel = useConversationStore((s) => s.setThinkingLevel);

  // Agent permission mode state
  const [agentPermissionMode, setAgentPermissionMode] = useState<string>(DEFAULT_AGENT_PERMISSION_MODE);
  const updateAgentEngine = useAgentStore((s) => s.updateEngine);

  // Agent working directory state
  const [agentCwd, setAgentCwd] = useState<string | null>(null);
  const [agentEngine, setAgentEngine] = useState<AgentEngineKind>(() => readDefaultAgentEngine());
  const [agentEngines, setAgentEngines] = useState<AgentEngineInfo[]>([]);
  const [loadingAgentEngines, setLoadingAgentEngines] = useState(false);

  // Context clear
  const insertContextClear = useConversationStore((s) => s.insertContextClear);
  const clearAllMessages = useConversationStore((s) => s.clearAllMessages);
  const updateConversation = useConversationStore((s) => s.updateConversation);
  const compressContext = useConversationStore((s) => s.compressContext);

  const activeConversation = conversations.find((c) => c.id === activeConversationId);
  const setEnabledMemoryNamespaceIds = useConversationStore((s) => s.setEnabledMemoryNamespaceIds);
  const getProjectProfile = useMemoryStore((s) => s.getProjectProfile);

  const toolbarIconButtonStyle = useMemo<React.CSSProperties>(() => ({
    width: 28,
    height: 28,
    minWidth: 28,
    padding: 0,
    display: 'inline-flex',
    alignItems: 'center',
    justifyContent: 'center',
  }), []);
  const toolbarDropdownButtonStyle = useMemo<React.CSSProperties>(() => ({
    height: 28,
    minWidth: 38,
    padding: '0 6px',
    display: 'inline-flex',
    alignItems: 'center',
    justifyContent: 'center',
    gap: 2,
  }), []);

  const setActivePage = useUIStore((s) => s.setActivePage);
  const setSettingsSection = useUIStore((s) => s.setSettingsSection);

  // Load search providers on mount
  useEffect(() => {
    if (searchProviders.length === 0) loadSearchProviders();
  }, [searchProviders.length, loadSearchProviders]);

  // Load MCP servers on mount
  useEffect(() => {
    if (mcpServers.length === 0) loadMcpServers();
  }, [mcpServers.length, loadMcpServers]);

  const loadAgentEngines = useCallback(async () => {
    setLoadingAgentEngines(true);
    try {
      const engines = await invoke<AgentEngineInfo[]>('agent_list_engines');
      setAgentEngines(engines);
    } catch (e) {
      console.warn('Failed to load agent engines:', e);
    } finally {
      setLoadingAgentEngines(false);
    }
  }, []);

  // Generate/load all engine menu items as soon as the input area starts.
  useEffect(() => {
    void loadAgentEngines();
  }, [loadAgentEngines]);

  // Fetch agent session and engine status on mount/conversation switch.
  // Keep the engine badge visible outside Agent mode, matching CodePilot.
  useEffect(() => {
    if (activeConversationId) {
      invoke('agent_get_session', { conversationId: activeConversationId })
        .then((session: any) => {
          if (session) {
            setAgentPermissionMode(session.permission_mode || DEFAULT_AGENT_PERMISSION_MODE);
            setAgentCwd(session.cwd || null);
            const sessionEngine = (session.engine_kind || session.engineKind || readDefaultAgentEngine()) as AgentEngineKind;
            setAgentEngine(sessionEngine);
            writeDefaultAgentEngine(sessionEngine);
          } else {
            setAgentPermissionMode(DEFAULT_AGENT_PERMISSION_MODE);
            setAgentEngine(readDefaultAgentEngine());
          }
        })
        .catch(() => {
          setAgentPermissionMode(DEFAULT_AGENT_PERMISSION_MODE);
          setAgentEngine(readDefaultAgentEngine());
        });
    } else {
      setAgentPermissionMode(DEFAULT_AGENT_PERMISSION_MODE);
      setAgentEngine(readDefaultAgentEngine());
    }
  }, [activeConversationId]);

  useEffect(() => {
    if (!activeConversation?.working_directory) return;
    let cancelled = false;
    void getProjectProfile(activeConversation.working_directory, activeConversation.project_name)
      .then((profile) => {
        if (cancelled || !profile) return;
        const current = useConversationStore.getState().enabledMemoryNamespaceIds;
        const nextIds = profile.embeddingProvider ? [profile.namespaceId] : [];
        if (current.length === nextIds.length && current.every((id, index) => id === nextIds[index])) return;
        setEnabledMemoryNamespaceIds(nextIds);
      })
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, [activeConversation?.working_directory, activeConversation?.project_name, getProjectProfile, setEnabledMemoryNamespaceIds]);

  // Draft persistence: save old draft & restore new when conversation changes
  useEffect(() => {
    const prev = prevConvIdRef.current;
    if (prev && prev !== activeConversationId) {
      const draft = valueRef.current;
      if (draft) _draftCache.set(prev, draft);
      else _draftCache.delete(prev);
    }
    setValue(activeConversationId ? _draftCache.get(activeConversationId) || '' : '');
    prevConvIdRef.current = activeConversationId ?? null;
  }, [activeConversationId]);

  // Save draft on unmount (navigating away from chat page)
  useEffect(() => {
    return () => {
      const convId = prevConvIdRef.current;
      if (convId && valueRef.current) {
        _draftCache.set(convId, valueRef.current);
      }
    };
  }, []);

  // Persist companion models per conversation in localStorage
  const companionStorageKey = activeConversationId ? `frogclaw:companion-models:${activeConversationId}` : null;

  // Load companion models when conversation changes
  useEffect(() => {
    if (!companionStorageKey) { setCompanionModels([]); return; }
    try {
      const saved = localStorage.getItem(companionStorageKey);
      setCompanionModels(saved ? JSON.parse(saved) : []);
    } catch { setCompanionModels([]); }
  }, [companionStorageKey]);

  // Pick up pending prompt text from welcome cards and send through the proper pipeline
  const pendingPromptText = useConversationStore((s) => s.pendingPromptText);
  useEffect(() => {
    if (!pendingPromptText) return;
    useConversationStore.getState().setPendingPromptText(null);
    const text = pendingPromptText;
    (async () => {
      try {
        await sendAgentMessage(text);
      } catch (e) {
        console.error('[InputArea] pendingPromptText send error:', e);
        messageApi.error(String(e));
      }
    })();
  }, [pendingPromptText]); // eslint-disable-line react-hooks/exhaustive-deps

  // Search dropdown menu items
  const searchMenuItems = useMemo(() => {
    const available = searchProviders;
    if (available.length === 0) {
      return [
        {
          key: '__empty',
          label: (
            <span style={{ color: token.colorTextSecondary, fontSize: 12 }}>
              {t('chat.search.noProviders')}
            </span>
          ),
          disabled: true,
        },
      ];
    }
    return available.map((p) => ({
      key: p.id,
      label: (
        <div className="flex items-center gap-2" style={{ minWidth: 140 }}>
          <Tag
            color="blue"
            style={{ margin: 0, fontSize: 11, lineHeight: '18px', padding: '0 6px', display: 'inline-flex', alignItems: 'center', gap: 3 }}
          >
            <SearchProviderTypeIcon type={p.providerType} size={14} />
            {PROVIDER_TYPE_LABELS[p.providerType] || p.providerType}
          </Tag>
          <span className="flex-1" style={{ fontSize: 13 }}>{p.name}</span>
          {searchEnabled && searchProviderId === p.id && (
            <Check size={14} style={{ color: token.colorPrimary }} />
          )}
        </div>
      ),
    }));
  }, [searchProviders, searchEnabled, searchProviderId, token, t]);

  const handleSearchMenuClick = useCallback(
    ({ key }: { key: string }) => {
      if (key === '__empty') return;
      setSearchEnabled(true);
      setSearchProviderId(key);
    },
    [setSearchEnabled, setSearchProviderId],
  );

  // Agent permission mode menu items
  const permissionModeItems = useMemo<MenuProps['items']>(() => [
    {
      key: 'default',
      label: t('common.permissionDefault'),
      icon: <Shield size={14} />,
    },
    {
      key: 'accept_edits',
      label: t('common.permissionAcceptEdits'),
      icon: <ShieldCheck size={14} style={{ color: '#1890ff' }} />,
    },
    {
      key: 'full_access',
      label: t('common.permissionFullAccess'),
      icon: <ShieldAlert size={14} style={{ color: '#ff4d4f' }} />,
    },
  ], [t]);

  const handlePermissionModeChange = useCallback(async (mode: string) => {
    if (!activeConversationId) return;

    const applyChange = async () => {
      try {
        await invoke('agent_update_session', {
          conversationId: activeConversationId,
          permissionMode: mode,
        });
        setAgentPermissionMode(mode);
      } catch (e) {
        console.warn('Failed to update permission mode:', e);
      }
    };

    if (mode === 'accept_edits' || mode === 'full_access') {
      const isFullAccess = mode === 'full_access';
      modal.confirm({
        title: isFullAccess
          ? t('agent.permissionFullAccessWarningTitle', '完全访问模式')
          : t('agent.permissionAcceptEditsWarningTitle', '允许编辑模式'),
        content: isFullAccess
          ? t('agent.permissionFullAccessWarning', 'Agent 将拥有完全访问权限，可以执行任何文件操作且不受路径限制。请确保你信任当前使用的模型和 System Prompt。')
          : t('agent.permissionAcceptEditsWarning', 'Agent 将自动批准文件编辑操作，无需逐一确认。请确保你了解潜在的安全风险。'),
        okText: t('common.confirm', '确认'),
        cancelText: t('common.cancel', '取消'),
        okButtonProps: isFullAccess ? { danger: true } : undefined,
        onOk: applyChange,
      });
    } else {
      await applyChange();
    }
  }, [activeConversationId, t]);

  const permissionModeIcon = useMemo(() => {
    switch (agentPermissionMode) {
      case 'accept_edits': return <ShieldCheck size={14} style={{ color: '#1890ff' }} />;
      case 'full_access': return <ShieldAlert size={14} style={{ color: '#ff4d4f' }} />;
      default: return <Shield size={14} />;
    }
  }, [agentPermissionMode]);

  const permissionModeLabel = useMemo(() => {
    switch (agentPermissionMode) {
      case 'accept_edits': return t('common.permissionAcceptEdits');
      case 'full_access': return t('common.permissionFullAccess');
      default: return t('common.permissionDefault');
    }
  }, [agentPermissionMode, t]);

  // Agent CWD helpers
  const abbreviatePath = useCallback((path: string): string => {
    const segments = path.replace(/\\/g, '/').split('/').filter(Boolean);
    if (segments.length <= 2) return path;
    return '.../' + segments.slice(-2).join('/');
  }, []);

  const handleSelectCwd = useCallback(async () => {
    if (!activeConversationId) return;
    try {
      const selected = await open({
        directory: true,
        multiple: false,
        title: t('common.selectDirectory'),
      });
      if (selected && typeof selected === 'string') {
        await invoke('agent_update_session', {
          conversationId: activeConversationId,
          cwd: selected,
        });
        setAgentCwd(selected);
      }
    } catch (e) {
      console.warn('Failed to select working directory:', e);
    }
  }, [activeConversationId, t]);

  const currentModel = React.useMemo(() => {
    if (activeConversation) {
      return findModelByIds(providers, activeConversation.provider_id, activeConversation.model_id);
    }

    if (settings.default_provider_id && settings.default_model_id) {
      const defaultModel = findModelByIds(providers, settings.default_provider_id, settings.default_model_id);
      if (defaultModel?.enabled) return defaultModel;
    }

    for (const provider of providers) {
      if (!provider.enabled) continue;
      const model = provider.models.find((item) => item.enabled);
      if (model) return model;
    }

    return null;
  }, [activeConversation, providers, settings.default_provider_id, settings.default_model_id]);

  const currentProviderType = useMemo<ProviderType | undefined>(() => {
    const providerId = currentModel?.provider_id ?? activeConversation?.provider_id;
    return providers.find((provider) => provider.id === providerId)?.provider_type;
  }, [activeConversation?.provider_id, currentModel?.provider_id, providers]);

  const nativeCliEngine = useMemo(
    () => resolveNativeCliEngine(currentProviderType, currentModel?.model_id ?? activeConversation?.model_id),
    [activeConversation?.model_id, currentModel?.model_id, currentProviderType],
  );

  const agentEngineMode = agentEngine === 'codex_app_server' || agentEngine === 'frog_agent' ? 'frog_agent' : 'native_cli';
  const nativeCliEngineInfo = useMemo(
    () => agentEngines.find((engine) => engine.kind === nativeCliEngine),
    [agentEngines, nativeCliEngine],
  );
  const nativeCliLabel = nativeCliEngineInfo?.displayName || formatAgentEngineLabel(nativeCliEngine);

  const applyAgentEngine = useCallback(async (nextEngine: AgentEngineKind, options?: { silent?: boolean }) => {
    if (!SUPPORTED_AGENT_ENGINE_KINDS.includes(nextEngine) || streaming) return false;
    if (nextEngine === agentEngine) return true;

    const previousEngine = agentEngine;
    setAgentEngine(nextEngine);
    writeDefaultAgentEngine(nextEngine);

    if (activeConversationId) {
      const updated = await updateAgentEngine(activeConversationId, nextEngine);
      if (!updated) {
        setAgentEngine(previousEngine);
        writeDefaultAgentEngine(previousEngine);
        if (!options?.silent) {
          messageApi.error(t('chat.engineSwitchFailed', 'Agent engine switch failed'));
        }
        return false;
      }
      const updatedEngine = (updated.engine_kind || updated.engineKind || nextEngine) as AgentEngineKind;
      setAgentEngine(updatedEngine);
      writeDefaultAgentEngine(updatedEngine);
    }

    if (!options?.silent) {
      const engineInfo = agentEngines.find((engine) => engine.kind === nextEngine);
      const statusMessage = engineInfo && !engineInfo.available
        ? (engineInfo.message || t('chat.engineUnavailable', 'Current CLI is unavailable'))
        : t('chat.engineSwitchSuccess', 'Agent engine switched');
      if (engineInfo && !engineInfo.available) {
        messageApi.warning(statusMessage);
      } else {
        messageApi.success(statusMessage);
      }
    }
    return true;
  }, [activeConversationId, agentEngine, agentEngines, messageApi, streaming, t, updateAgentEngine]);

  const handleAgentEngineModeChange = useCallback((value: string | number) => {
    const nextEngine = value === 'native_cli' ? nativeCliEngine : 'codex_app_server';
    const runChange = () => applyAgentEngine(nextEngine);
    if (activeConversationId && messages.length > 0 && nextEngine !== agentEngine) {
      modal.confirm({
        title: t('chat.switchEngineTitle', 'Switch Agent Engine'),
        content: t('chat.switchEngineContent', 'Switching engines starts a new runtime context. Existing messages are kept.'),
        okText: t('common.confirm', 'Confirm'),
        cancelText: t('common.cancel', 'Cancel'),
        onOk: runChange,
      });
    } else {
      void runChange();
    }
  }, [activeConversationId, agentEngine, applyAgentEngine, messages.length, modal, nativeCliEngine, t]);

  useEffect(() => {
    if (agentEngine !== 'codex_app_server' && agentEngine !== 'frog_agent' && agentEngine !== nativeCliEngine && !streaming) {
      void applyAgentEngine(nativeCliEngine, { silent: true });
    }
  }, [agentEngine, applyAgentEngine, nativeCliEngine, streaming]);

  const reasoningProfile = useMemo(
    () => resolveReasoningProfile(currentProviderType, currentModel),
    [currentModel, currentProviderType],
  );

  const thinkingOptions = useMemo(
    () => reasoningProfile.options.map((option) => ({
      ...option,
      label: t(option.labelKey, option.fallbackLabel),
    })),
    [reasoningProfile, t],
  );

  const selectedThinkingKey = useMemo(() => {
    const legacyKey = thinkingLevel === null
      ? legacyThinkingBudgetToOptionKey(reasoningProfile, thinkingBudget)
      : null;
    return coerceReasoningOptionKey(reasoningProfile, thinkingLevel ?? legacyKey);
  }, [reasoningProfile, thinkingBudget, thinkingLevel]);

  const selectedThinkingOption = useMemo(
    () => thinkingOptions.find((opt) => opt.key === selectedThinkingKey) ?? thinkingOptions[0],
    [selectedThinkingKey, thinkingOptions],
  );

  const thinkingIcon = useMemo(() => {
    switch (selectedThinkingOption.icon) {
      case 'off': return <CircleOff size={14} />;
      case 'low': return <SignalLow size={14} />;
      case 'medium': return <SignalMedium size={14} />;
      case 'high': return <SignalHigh size={14} />;
      case 'xhigh': return <Signal size={14} />;
      case 'max': return <Signal size={14} />;
      default: return <Atom size={14} />;
    }
  }, [selectedThinkingOption.icon]);

  // Context token usage calculation
  const getCompressionSummary = useConversationStore((s) => s.getCompressionSummary);
  const [summaryTokenCount, setSummaryTokenCount] = useState<number>(0);

  useEffect(() => {
    if (!activeConversationId || !activeConversation?.context_compression) {
      setSummaryTokenCount(0);
      return;
    }
    getCompressionSummary(activeConversationId).then((s) => {
      setSummaryTokenCount(s?.token_count ?? 0);
    });
  }, [activeConversationId, activeConversation?.context_compression, getCompressionSummary, messages]);

  // TODO: Token estimation only considers loaded messages. When hasOlderMessages is true
  // and no context-clear marker is found, the token estimate will be lower than actual.
  // A proper fix would require the backend to return total token counts.
  const contextTokenUsage = useMemo(() => {
    const maxTokens = currentModel?.max_tokens;
    if (!maxTokens) return null;

    // Count message tokens (only after last marker)
    const activeMessages = messages.filter((m) => m.is_active !== false && !m.content.startsWith('%%ERROR%%'));
    const lastMarkerIdx = activeMessages.reduce((maxIdx, m, i) => {
      if (m.content === '<!-- context-clear -->' || m.content === '<!-- context-compressed -->') return i;
      return maxIdx;
    }, -1);
    const effectiveMessages = lastMarkerIdx === -1 ? activeMessages : activeMessages.slice(lastMarkerIdx + 1);
    let usedTokens = effectiveMessages.reduce(
      (sum, m) => sum + estimateMessageTokens(m.role, m.content), 0,
    );

    // Add system prompt
    if (activeConversation?.system_prompt) {
      usedTokens += estimateTokens(activeConversation.system_prompt) + 4;
    }

    // Add summary tokens
    usedTokens += summaryTokenCount;

    const percent = Math.min(Math.round((usedTokens / maxTokens) * 100), 100);
    return { usedTokens, maxTokens, percent };
  }, [messages, currentModel?.max_tokens, activeConversation?.system_prompt, summaryTokenCount]);

  const { hasRealtimeVoice, hasReasoning, hasVision } = React.useMemo(() => ({
    hasRealtimeVoice: activeConversation
      ? !!findModelByIds(providers, activeConversation.provider_id, activeConversation.model_id)?.capabilities.includes('RealtimeVoice')
      : false,
    hasReasoning: supportsReasoning(currentModel),
    hasVision: modelHasCapability(currentModel, 'Vision'),
  }), [activeConversation, currentModel, providers]);

  const voiceConfig: RealtimeConfig = React.useMemo(
    () => ({
      model_id: activeConversation?.model_id ?? '',
      voice: null,
      audio_format: { sample_rate: 24000, channels: 1, encoding: 'Pcm16' },
    }),
    [activeConversation?.model_id],
  );

  const initializeAgentSession = useCallback(async () => {
    if (!activeConversation) return;

    try {
      if (activeConversation.mode !== 'agent') {
        await updateConversation(activeConversation.id, { mode: 'agent' });
      }
    } catch (e) {
      console.warn('Failed to persist default agent mode:', e);
    }

    try {
      // Clear multi-model companion models - not applicable in agent mode.
      if (companionModels.length > 0) {
        setCompanionModels([]);
        if (companionStorageKey) localStorage.removeItem(companionStorageKey);
      }
      const session = await invoke<{ cwd: string | null; permission_mode?: string; permissionMode?: string; engine_kind?: AgentEngineKind; engineKind?: AgentEngineKind }>('agent_update_session', {
        conversationId: activeConversation.id,
        permissionMode: DEFAULT_AGENT_PERMISSION_MODE,
        engineKind: agentEngine,
      });
      setAgentPermissionMode(session.permission_mode || session.permissionMode || DEFAULT_AGENT_PERMISSION_MODE);
      const sessionEngine = (session.engine_kind || session.engineKind || agentEngine) as AgentEngineKind;
      setAgentEngine(sessionEngine);
      writeDefaultAgentEngine(sessionEngine);
      if (activeConversation.working_directory) {
        await invoke('agent_update_session', {
          conversationId: activeConversation.id,
          cwd: activeConversation.working_directory,
          permissionMode: DEFAULT_AGENT_PERMISSION_MODE,
          engineKind: sessionEngine,
        });
        setAgentCwd(activeConversation.working_directory);
      } else if (!session.cwd) {
        const workspacePath = await invoke<string>('agent_ensure_workspace', {
          conversationId: activeConversation.id,
        });
        await invoke('agent_update_session', {
          conversationId: activeConversation.id,
          cwd: workspacePath,
          permissionMode: DEFAULT_AGENT_PERMISSION_MODE,
          engineKind: sessionEngine,
        });
        setAgentCwd(workspacePath);
      } else {
        setAgentCwd(session.cwd);
      }
    } catch (e) {
      console.warn('Failed to init agent session:', e);
    }
  }, [activeConversation, updateConversation, companionModels, companionStorageKey, agentEngine]);

  useEffect(() => {
    void initializeAgentSession();
  }, [initializeAgentSession]);

  const enabledMcpServers = useMemo(
    () => mcpServers.filter((server) => server.enabled),
    [mcpServers],
  );

  const handleClearConversationFromMenu = useCallback(() => {
    if (!activeConversationId || streaming || messages.length === 0) return;
    modal.confirm({
      title: t('chat.clearConversationConfirmTitle'),
      content: t('chat.clearConversationConfirmContent'),
      okButtonProps: { danger: true },
      okText: t('common.confirm'),
      cancelText: t('common.cancel'),
      onOk: async () => {
        await clearAllMessages();
      },
    });
  }, [activeConversationId, clearAllMessages, messages.length, modal, streaming, t]);

  const contextMenuItems = useMemo<MenuProps['items']>(() => {
    const items: MenuProps['items'] = [];

    items.push({
      key: 'mcp',
      icon: <Plug size={14} />,
      label: t('chat.mcp.title'),
      children: enabledMcpServers.length > 0
        ? enabledMcpServers.map((server) => ({
            key: `mcp:${server.id}`,
            label: server.name,
            icon: enabledMcpServerIds.includes(server.id) ? <Check size={14} /> : undefined,
          }))
        : [
            {
              key: 'mcp-settings',
              label: t('chat.mcp.goConfig'),
            },
          ],
    });

    items.push(
      { type: 'divider' },
      {
        key: 'auto',
        icon: activeConversation?.context_compression ? <ZapOff size={14} /> : <Zap size={14} />,
        label: activeConversation?.context_compression
          ? t('chat.disableAutoCompression')
          : t('chat.enableAutoCompression'),
        disabled: !activeConversationId,
      },
      {
        key: 'manual',
        icon: <Shrink size={14} />,
        label: t('chat.manualCompress'),
        disabled: !activeConversationId || streaming || compressing || messages.length === 0,
      },
      { type: 'divider' },
      {
        key: 'clear-context',
        icon: <Scissors size={14} />,
        label: shortcutHint(t('chat.clearContext'), 'clearContext'),
        disabled: !activeConversationId || streaming || messages.length === 0 || messages[messages.length - 1]?.content === '<!-- context-clear -->',
      },
      {
        key: 'clear-conversation',
        icon: <Eraser size={14} />,
        label: shortcutHint(t('chat.clearConversation'), 'clearConversationMessages'),
        danger: true,
        disabled: !activeConversationId || streaming || messages.length === 0,
      },
    );

    return items;
  }, [
    activeConversation?.context_compression,
    activeConversationId,
    compressing,
    enabledMcpServerIds,
    enabledMcpServers,
    messages,
    shortcutHint,
    streaming,
    t,
  ]);

  const handleContextMenuClick = useCallback<NonNullable<MenuProps['onClick']>>(
    async ({ key }) => {
      const keyText = String(key);
      if (keyText.startsWith('mcp:')) {
        toggleMcpServer(keyText.slice('mcp:'.length));
        return;
      }

      if (keyText === 'mcp-settings') {
        setSettingsSection('mcpServers');
        setActivePage('settings');
        return;
      }

      if (keyText === 'auto') {
        if (!activeConversationId || !activeConversation) return;
        updateConversation(activeConversationId, { context_compression: !activeConversation.context_compression });
        return;
      }

      if (keyText === 'manual') {
        if (!activeConversationId) return;
        try {
          await compressContext();
          messageApi.success(t('chat.compressSuccess'));
        } catch {
          messageApi.error(t('chat.compressFailed'));
        }
        return;
      }

      if (keyText === 'clear-context') {
        if (activeConversationId && !streaming) void insertContextClear();
        return;
      }

      if (keyText === 'clear-conversation') {
        handleClearConversationFromMenu();
      }
    },
    [
      activeConversation,
      activeConversationId,
      compressContext,
      handleClearConversationFromMenu,
      insertContextClear,
      messageApi,
      setActivePage,
      setSettingsSection,
      streaming,
      t,
      toggleMcpServer,
      updateConversation,
    ],
  );

  const handleSend = useCallback(async () => {
    const trimmed = value.trim();
    if (!trimmed) return;

    const submittedFiles = attachedFiles;

    try {
      if (!activeConversationId) {
        let provider = settings.default_provider_id
          ? providers.find((p) => p.id === settings.default_provider_id && p.enabled)
          : undefined;
        let model = provider?.models.find(
          (m) => m.model_id === settings.default_model_id && m.enabled,
        );
        if (!provider || !model) {
          provider = providers.find((p) => p.enabled && p.models.some((m) => m.enabled));
          model = provider?.models.find((m) => m.enabled);
        }
        if (!provider || !model) {
          messageApi.warning(t('chat.noModelsAvailable'));
          return;
        }
        await createConversation(trimmed.slice(0, 30), model.model_id, provider.id);
      }

      let attachments: AttachmentInput[] | undefined;
      if (submittedFiles.length > 0) {
        attachments = await Promise.all(submittedFiles.map(fileToAttachmentInput));
      }

      setValue('');
      setAttachedFiles([]);
      // Reset textarea height and drag state after clearing content
      hasUserResizedRef.current = false;
      setUserMinHeight(INITIAL_MIN_HEIGHT);
      userMinHeightRef.current = INITIAL_MIN_HEIGHT;
      requestAnimationFrame(() => {
        if (textareaRef.current) {
          textareaRef.current.style.height = 'auto';
        }
      });
      await sendAgentMessage(trimmed, attachments);
    } catch (e) {
      setValue((current) => current || trimmed);
      setAttachedFiles((current) => (current.length > 0 ? current : submittedFiles));
      console.error('[handleSend] error:', e);
      messageApi.error(String(e));
      // Re-expand textarea after restoring content
      requestAnimationFrame(() => {
        const textarea = textareaRef.current;
        if (textarea) {
          textarea.style.height = 'auto';
          const desired = hasUserResizedRef.current
            ? userMinHeightRef.current
            : Math.max(textarea.scrollHeight, userMinHeightRef.current);
          textarea.style.height = Math.min(desired, ABSOLUTE_MAX_HEIGHT) + 'px';
        }
      });
    }
  }, [value, attachedFiles, sendAgentMessage, activeConversationId, providers, settings, createConversation, messageApi, t]);

  const handleFillLastMessage = useCallback(() => {
    if (streaming) return;
    const lastUserMessage = [...messages]
      .reverse()
      .find((message) => message.role === 'user' && message.status !== 'error');
    if (!lastUserMessage?.content) return;
    setValue(lastUserMessage.content);
    hasUserResizedRef.current = false;
    requestAnimationFrame(() => {
      const textarea = textareaRef.current;
      if (!textarea) return;
      textarea.focus();
      textarea.style.height = 'auto';
      const desired = Math.max(textarea.scrollHeight, userMinHeightRef.current);
      textarea.style.height = Math.min(desired, ABSOLUTE_MAX_HEIGHT) + 'px';
    });
  }, [messages, streaming]);

  const handleCancel = useCallback(() => {
    cancelCurrentStream();
  }, [cancelCurrentStream]);

  const handleFileSelect = useCallback(() => {
    fileInputRef.current?.click();
  }, []);

  const handleFileChange = useCallback((e: React.ChangeEvent<HTMLInputElement>) => {
    const files = e.target.files;
    if (files) {
      setAttachedFiles((prev) => [...prev, ...Array.from(files)]);
    }
    if (fileInputRef.current) {
      fileInputRef.current.value = '';
    }
  }, []);

  const removeFile = useCallback((index: number) => {
    setAttachedFiles((prev) => prev.filter((_, i) => i !== index));
  }, []);

  const handlePaste = useCallback((e: React.ClipboardEvent<HTMLTextAreaElement>) => {
    if (!hasVision) return;
    const items = e.clipboardData?.items;
    if (!items) return;
    const files: File[] = [];
    for (const item of items) {
      if (item.kind === 'file') {
        const file = item.getAsFile();
        if (file) files.push(file);
      }
    }
    if (files.length > 0) {
      e.preventDefault();
      setAttachedFiles((prev) => [...prev, ...files]);
    }
  }, [hasVision]);

  // Drag-and-drop overlay (Tauri native)
  const [isDragging, setIsDragging] = useState(false);

  useEffect(() => {
    if (!hasVision) return;

    let unlisten: (() => void) | undefined;

    (async () => {
      const { getCurrentWebview } = await import('@tauri-apps/api/webview');
      const { readFile } = await import('@tauri-apps/plugin-fs');

      unlisten = await getCurrentWebview().onDragDropEvent(async (event) => {
        const { type } = event.payload;
        if (type === 'enter') {
          setIsDragging(true);
        } else if (type === 'leave') {
          setIsDragging(false);
        } else if (type === 'drop') {
          setIsDragging(false);
          const { paths } = event.payload;
          const files: File[] = [];
          for (const filePath of paths) {
            try {
              const fileName = filePath.split(/[\\/]/).pop() || 'file';
              const ext = fileName.split('.').pop()?.toLowerCase() || '';
              const mimeMap: Record<string, string> = {
                png: 'image/png', jpg: 'image/jpeg', jpeg: 'image/jpeg',
                gif: 'image/gif', webp: 'image/webp', svg: 'image/svg+xml',
                bmp: 'image/bmp', ico: 'image/x-icon',
                pdf: 'application/pdf', txt: 'text/plain',
                json: 'application/json', csv: 'text/csv',
                md: 'text/markdown', html: 'text/html',
                js: 'text/javascript', ts: 'text/typescript',
                zip: 'application/zip',
              };
              const mimeType = mimeMap[ext] || 'application/octet-stream';
              const bytes = await readFile(filePath);
              files.push(new File([bytes], fileName, { type: mimeType }));
            } catch (err) {
              console.error('[drag-drop] Failed to read file:', filePath, err);
            }
          }
          if (files.length > 0) {
            setAttachedFiles((prev) => [...prev, ...files]);
          }
        }
      });
    })();

    return () => {
      unlisten?.();
    };
  }, [hasVision]);

  const handleKeyDown = useCallback(
    (e: React.KeyboardEvent<HTMLTextAreaElement>) => {
      if (e.nativeEvent.isComposing || e.key === 'Process' || e.keyCode === 229) {
        return;
      }
      const sendShortcut = getShortcutBinding(settings, 'sendMessage');
      if (matchesShortcutEvent(e.nativeEvent, sendShortcut)) {
        e.preventDefault();
        e.stopPropagation();
        handleSend();
      }
    },
    [handleSend, settings],
  );

  // Auto-resize textarea: height = max(userMinHeight, contentHeight), capped at ABSOLUTE_MAX
  // When user has explicitly dragged to resize, lock height to userMinHeight (content scrolls)
  const autoResizeTextarea = useCallback((el: HTMLTextAreaElement) => {
    el.style.height = 'auto';
    const desired = hasUserResizedRef.current
      ? userMinHeightRef.current
      : Math.max(el.scrollHeight, userMinHeightRef.current);
    el.style.height = Math.min(desired, ABSOLUTE_MAX_HEIGHT) + 'px';
  }, []);

  const handleInput = useCallback((e: React.ChangeEvent<HTMLTextAreaElement>) => {
    setValue(e.target.value);
    autoResizeTextarea(e.target);
  }, [autoResizeTextarea]);

  // Drag-to-resize: changes userMinHeight so the textarea grows even with short content
  const handleResizeMouseDown = useCallback((e: React.MouseEvent) => {
    e.preventDefault();
    const textarea = textareaRef.current;
    const startHeight = textarea ? textarea.offsetHeight : userMinHeightRef.current;
    dragStateRef.current = { startY: e.clientY, startH: startHeight };
    const onMouseMove = (ev: MouseEvent) => {
      if (!dragStateRef.current) return;
      const delta = dragStateRef.current.startY - ev.clientY;
      const newH = Math.max(INITIAL_MIN_HEIGHT, Math.min(ABSOLUTE_MAX_HEIGHT, dragStateRef.current.startH + delta));
      hasUserResizedRef.current = true;
      setUserMinHeight(newH);
      userMinHeightRef.current = newH;
      if (textarea) {
        textarea.style.height = newH + 'px';
      }
    };
    const onMouseUp = () => {
      dragStateRef.current = null;
      document.removeEventListener('mousemove', onMouseMove);
      document.removeEventListener('mouseup', onMouseUp);
      document.body.style.cursor = '';
      document.body.style.userSelect = '';
    };
    document.addEventListener('mousemove', onMouseMove);
    document.addEventListener('mouseup', onMouseUp);
    document.body.style.cursor = 'ns-resize';
    document.body.style.userSelect = 'none';
  }, []);

  // Listen for Escape to close voice overlay
  React.useEffect(() => {
    const onEscape = () => setVoiceCallVisible(false);
    window.addEventListener('frogclaw:escape', onEscape);
    return () => window.removeEventListener('frogclaw:escape', onEscape);
  }, []);

  React.useEffect(() => {
    const onFillLast = () => handleFillLastMessage();
    const onClearContext = () => {
      if (activeConversationId && !streaming) {
        void insertContextClear();
      }
    };
    const onClearConversation = () => {
      if (!activeConversationId || streaming || messages.length === 0) return;
      modal.confirm({
        title: t('chat.clearConversationConfirmTitle'),
        content: t('chat.clearConversationConfirmContent'),
        okButtonProps: { danger: true },
        okText: t('common.confirm'),
        cancelText: t('common.cancel'),
        onOk: async () => {
          await clearAllMessages();
        },
      });
    };

    window.addEventListener('frogclaw:fill-last-message', onFillLast);
    window.addEventListener('frogclaw:clear-context', onClearContext);
    window.addEventListener('frogclaw:clear-conversation-messages', onClearConversation);
    return () => {
      window.removeEventListener('frogclaw:fill-last-message', onFillLast);
      window.removeEventListener('frogclaw:clear-context', onClearContext);
      window.removeEventListener('frogclaw:clear-conversation-messages', onClearConversation);
    };
  }, [
    activeConversationId,
    clearAllMessages,
    handleFillLastMessage,
    insertContextClear,
    messages.length,
    modal,
    streaming,
    t,
  ]);

  // Listen for "fill input" events from GlobalCopyMenu
  React.useEffect(() => {
    const onFillInput = (e: Event) => {
      const text = (e as CustomEvent).detail;
      if (typeof text !== 'string' || !text) return;
      setValue((prev) => (prev ? prev + '\n' + text : text));
      requestAnimationFrame(() => {
        const textarea = textareaRef.current;
        if (!textarea) return;
        textarea.focus();
        textarea.style.height = 'auto';
        const desired = hasUserResizedRef.current
          ? userMinHeightRef.current
          : Math.max(textarea.scrollHeight, userMinHeightRef.current);
        textarea.style.height = Math.min(desired, ABSOLUTE_MAX_HEIGHT) + 'px';
      });
    };
    window.addEventListener('frogclaw:fill-input', onFillInput);
    return () => window.removeEventListener('frogclaw:fill-input', onFillInput);
  }, []);

  // Mode switching is intentionally disabled; FrogClaw chat now runs in Agent mode by default.
  React.useEffect(() => {
    const onToggleMode = () => void initializeAgentSession();
    window.addEventListener('frogclaw:toggle-mode', onToggleMode);
    return () => window.removeEventListener('frogclaw:toggle-mode', onToggleMode);
  }, [initializeAgentSession]);

  return (
    <div
      className="px-4 pb-2 pt-2"
      style={{
        background: token.colorBgContainer,
      }}
    >
      <input
        ref={fileInputRef}
        type="file"
        multiple
        style={{ display: 'none' }}
        onChange={handleFileChange}
      />

      {/* Attachment preview */}
      {attachedFiles.length > 0 && (
        <div className="mx-auto flex max-w-3xl flex-wrap gap-2 mb-2">
          {attachedFiles.map((file, idx) => (
            <span
              key={`${file.name}-${idx}`}
              className="inline-flex items-center gap-1 px-2 py-1 text-xs"
              style={{
                backgroundColor: token.colorFillTertiary,
                borderRadius: token.borderRadius,
              }}
            >
              {file.name}
              <Trash2
                size={14}
                className="cursor-pointer"
                style={{ color: token.colorTextSecondary }}
                onClick={() => removeFile(idx)}
              />
            </span>
          ))}
        </div>
      )}

      <div className="mx-auto max-w-3xl">
      {/* Main input container */}
      <div
        ref={containerRef}
        style={{
          border: `1px solid ${token.colorBorderSecondary}`,
          borderRadius: 12,
          backgroundColor: token.colorBgElevated,
          boxShadow: token.boxShadowTertiary,
          overflow: 'hidden',
        }}
      >
        {/* Drag-to-resize handle */}
        <div
          onMouseDown={handleResizeMouseDown}
          style={{
            height: 6,
            cursor: 'ns-resize',
            display: 'flex',
            alignItems: 'center',
            justifyContent: 'center',
            flexShrink: 0,
          }}
        >
          <GripHorizontal size={12} style={{ color: token.colorTextQuaternary, opacity: 0.4 }} />
        </div>
        {/* Textarea */}
        <textarea
          className="frogclaw-input-textarea"
          ref={textareaRef}
          value={value}
          onChange={handleInput}
          onKeyDown={handleKeyDown}
          onPaste={handlePaste}
          placeholder={t('chat.inputPlaceholder')}
          rows={1}
          style={{
            width: '100%',
            border: 'none',
            outline: 'none',
            resize: 'none',
            padding: '6px 14px 8px',
            fontSize: token.fontSize,
            lineHeight: 1.6,
            backgroundColor: 'transparent',
            color: token.colorText,
            fontFamily: 'inherit',
            minHeight: userMinHeight,
            maxHeight: ABSOLUTE_MAX_HEIGHT,
            overflowY: 'auto',
          }}
        />

        {/* Bottom action bar */}
        <div className="flex items-center justify-between gap-2 px-2 pb-2">
          <div className="flex min-w-0 flex-1 items-center gap-1">
            {searchEnabled ? (
              <Tooltip title={t('chat.search.title')}>
                <Button
                  type="text"
                  size="small"
                  icon={<Globe size={16} />}
                  style={{ ...toolbarIconButtonStyle, color: token.colorPrimary }}
                  onClick={() => {
                    setSearchEnabled(false);
                    setSearchProviderId(null);
                  }}
                />
              </Tooltip>
            ) : (
              <Dropdown
                trigger={['click']}
                placement="topLeft"
                menu={{ items: searchMenuItems, onClick: handleSearchMenuClick }}
                open={searchDropdownOpen}
                onOpenChange={setSearchDropdownOpen}
              >
                <Tooltip title={t('chat.search.title')} open={searchDropdownOpen ? false : undefined}>
                  <Button
                    type="text"
                    size="small"
                    icon={<Globe size={16} />}
                    style={toolbarIconButtonStyle}
                  />
                </Tooltip>
              </Dropdown>
            )}
            {hasVision && (
              <Tooltip title={t('chat.attachFile')}>
                <Button
                  type="text"
                  size="small"
                  icon={<Paperclip size={16} />}
                  style={toolbarIconButtonStyle}
                  onClick={handleFileSelect}
                />
              </Tooltip>
            )}
            {hasReasoning && (
              <Dropdown
                trigger={['click']}
                placement="topLeft"
                menu={{
                  items: thinkingOptions.map((option) => ({
                    key: option.key,
                    label: option.label,
                    icon: selectedThinkingOption.key === option.key ? <Check size={14} /> : undefined,
                  })),
                  onClick: ({ key }) => {
                    const selected = thinkingOptions.find((option) => option.key === key);
                    if (!selected) return;
                    setThinkingLevel(selected.key === 'default' ? null : selected.key);
                    if (selected.key === 'default') setThinkingBudget(null);
                  },
                  selectable: true,
                  selectedKeys: [selectedThinkingOption.key],
                }}
              >
                <Tooltip title={`${t('chat.thinkingIntensity')}: ${selectedThinkingOption.label}`}>
                  <Button
                    aria-label={t('chat.thinkingIntensity')}
                    type="text"
                    size="small"
                    icon={thinkingIcon}
                    style={{
                      ...toolbarIconButtonStyle,
                      ...(
                        selectedThinkingOption.key === 'off' || selectedThinkingOption.key === 'none'
                          ? { color: token.colorError }
                          : selectedThinkingOption.key !== 'default'
                            ? { color: token.colorPrimary }
                            : {}
                      ),
                    }}
                  />
                </Tooltip>
              </Dropdown>
            )}
            <Dropdown
              menu={{ items: contextMenuItems, onClick: handleContextMenuClick }}
              trigger={['click']}
              placement="topLeft"
            >
              <Tooltip title={t('chat.contextCompression')}>
                <Button
                  aria-label={t('chat.contextCompression')}
                  type="text"
                  size="small"
                  icon={<Zap size={16} />}
                  loading={compressing}
                  style={{
                    ...toolbarDropdownButtonStyle,
                    ...(activeConversation?.context_compression ? { color: token.colorPrimary } : {}),
                  }}
                >
                  <ChevronDown size={12} />
                </Button>
              </Tooltip>
            </Dropdown>
            {hasRealtimeVoice && (
              <Tooltip title={`${t('voice.startCall')} (not implemented)`}>
                <Button
                  type="text"
                  size="small"
                  icon={<Mic size={16} />}
                  style={toolbarIconButtonStyle}
                  disabled
                />
              </Tooltip>
            )}
            <div style={{ width: 1, height: 18, background: token.colorBorderSecondary, margin: '0 5px' }} />
            <ModelSelector />
          </div>
          <div className="flex shrink-0 items-center gap-2">
            {streaming ? (
              <Button
                shape="circle"
                size="small"
                danger
                icon={<Square size={14} />}
                onClick={handleCancel}
              />
            ) : (
              <Button
                type="primary"
                shape="circle"
                size="small"
                icon={<ArrowUp size={14} />}
                onClick={handleSend}
                disabled={!value.trim()}
              />
            )}
          </div>
        </div>
      </div>

      {/* Agent controls bar below input container */}
      <div className="flex items-center justify-between gap-2 px-1 pt-1.5">
        <div
          className="flex min-w-0 shrink-0 flex-col items-stretch gap-1"
          style={{
            padding: 3,
            width: 520,
            borderRadius: token.borderRadiusLG,
            border: `1px solid ${token.colorBorderSecondary}`,
            background: token.colorBgContainer,
            boxShadow: 'inset 0 1px 0 rgba(255,255,255,0.04)',
          }}
        >
          <div style={{ display: 'flex', alignItems: 'center', gap: 6, width: '100%' }}>
            <span style={{ width: 34, fontSize: 11, fontWeight: 700, color: token.colorTextSecondary, whiteSpace: 'nowrap', paddingLeft: 4 }}>{t('chat.engineLabel', '引擎')}</span>
            <div style={{ flex: 1, display: 'inline-flex', alignItems: 'center', gap: 4, padding: 2, borderRadius: token.borderRadiusLG, background: token.colorFillQuaternary }}>
              <Button
                size="small"
                type={agentEngineMode === 'frog_agent' ? 'primary' : 'text'}
                disabled={streaming || loadingAgentEngines}
                icon={<Atom size={12} />}
                onClick={() => handleAgentEngineModeChange('codex_app_server')}
                style={{
                  height: 26,
                  flex: 1,
                  fontSize: 12,
                  fontWeight: 700,
                  color: agentEngineMode === 'frog_agent' ? token.colorWhite : token.colorText,
                  background: agentEngineMode === 'frog_agent' ? token.colorPrimary : 'transparent',
                  boxShadow: agentEngineMode === 'frog_agent' ? token.boxShadowSecondary : 'none',
                }}
              >
                AIAgent
              </Button>
              <Button
                size="small"
                type={agentEngineMode === 'native_cli' ? 'primary' : 'text'}
                disabled={streaming || loadingAgentEngines}
                icon={<Terminal size={12} />}
                onClick={() => handleAgentEngineModeChange('native_cli')}
                style={{
                  height: 26,
                  flex: 1.55,
                  fontSize: 12,
                  fontWeight: 700,
                  color: agentEngineMode === 'native_cli' ? token.colorWhite : token.colorText,
                  background: agentEngineMode === 'native_cli' ? token.colorPrimary : 'transparent',
                  boxShadow: agentEngineMode === 'native_cli' ? token.boxShadowSecondary : 'none',
                }}
              >
                <span style={{ display: 'inline-flex', alignItems: 'center', gap: 6 }}>
                  {t('chat.nativeCli', 'Native CLI')}
                  <span style={{ fontSize: 10, color: agentEngineMode === 'native_cli' ? token.colorWhite : token.colorTextTertiary, fontWeight: 700 }}>
                    {nativeCliLabel}
                  </span>
                </span>
              </Button>
            </div>
            <div style={{ display: 'flex', alignItems: 'center', gap: 4, minWidth: 0 }}>
              <Tooltip title={agentCwd || t('common.workingDirectory')}>
                <Button
                  type="text"
                  size="small"
                  icon={<FolderOpen size={14} />}
                  onClick={handleSelectCwd}
                  style={{ display: 'flex', alignItems: 'center', gap: 4, maxWidth: 150, fontSize: 12 }}
                >
                  <span style={{ overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>
                    {agentCwd ? abbreviatePath(agentCwd) : t('common.selectDirectory')}
                  </span>
                </Button>
              </Tooltip>
              {agentCwd && (
                <Tooltip title={t('common.openDirectory', 'Open directory')}>
                  <Button
                    type="text"
                    size="small"
                    icon={<ExternalLink size={14} />}
                    onClick={async () => {
                      try {
                        const { revealItemInDir } = await import('@tauri-apps/plugin-opener');
                        await revealItemInDir(agentCwd);
                      } catch (e) {
                        console.warn('Failed to open directory:', e);
                      }
                    }}
                    style={{ fontSize: 12, minWidth: 'auto', padding: '0 4px' }}
                  />
                </Tooltip>
              )}
            </div>
          </div>
        </div>
        <div className="ml-auto flex shrink-0 items-center gap-2">
          <Dropdown
            menu={{
              items: permissionModeItems,
              selectedKeys: [agentPermissionMode],
              onClick: ({ key }) => handlePermissionModeChange(key),
            }}
            trigger={['click']}
            placement="topRight"
          >
            <Button
              type="text"
              size="small"
              icon={permissionModeIcon}
              style={{
                display: 'flex', alignItems: 'center', gap: 4, fontSize: 12,
                ...(agentPermissionMode === 'full_access' ? { color: '#ff4d4f' } : {}),
              }}
            >
              {permissionModeLabel}
            </Button>
          </Dropdown>
          {contextCount > 0 && (
            <span style={{ fontSize: 11, color: token.colorTextSecondary }}>
              {contextCount} {t('chat.contextMessages')}
            </span>
          )}
          {contextTokenUsage && (() => {
            const r = 8, stroke = 2.5, size = (r + stroke) * 2;
            const circ = 2 * Math.PI * r;
            const offset = circ * (1 - contextTokenUsage.percent / 100);
            const color = contextTokenUsage.percent > 80
              ? token.colorError
              : contextTokenUsage.percent > 60
                ? token.colorWarning
                : token.colorPrimary;
            return (
              <Popover
                content={
                  <span style={{ fontSize: 12 }}>
                    {contextTokenUsage.usedTokens.toLocaleString()} / {contextTokenUsage.maxTokens.toLocaleString()} tokens ({contextTokenUsage.percent}%)
                  </span>
                }
              >
                <svg width={size} height={size} style={{ display: 'block', cursor: 'pointer' }}>
                  <circle cx={r + stroke} cy={r + stroke} r={r} fill="none" stroke={token.colorBorderSecondary} strokeWidth={stroke} />
                  <circle
                    cx={r + stroke} cy={r + stroke} r={r}
                    fill="none" stroke={color} strokeWidth={stroke}
                    strokeDasharray={circ} strokeDashoffset={offset}
                    strokeLinecap="round"
                    transform={`rotate(-90 ${r + stroke} ${r + stroke})`}
                  />
                </svg>
              </Popover>
            );
          })()}
        </div>
      </div>
      </div>

      {hasRealtimeVoice && (
        <VoiceCall
          visible={voiceCallVisible}
          onClose={() => setVoiceCallVisible(false)}
          config={voiceConfig}
        />
      )}

      {/* Drag-and-drop overlay */}
      {isDragging && (
        <div
          style={{
            position: 'fixed',
            inset: 0,
            zIndex: 9999,
            display: 'flex',
            alignItems: 'center',
            justifyContent: 'center',
            backgroundColor: 'rgba(0, 0, 0, 0.45)',
            backdropFilter: 'blur(4px)',
          }}
        >
          <div
            style={{
              display: 'flex',
              flexDirection: 'column',
              alignItems: 'center',
              gap: 12,
              padding: '40px 60px',
              borderRadius: 16,
              border: `2px dashed ${token.colorPrimary}`,
              backgroundColor: token.colorBgElevated,
            }}
          >
            <Upload size={48} style={{ color: token.colorPrimary }} />
            <span style={{ fontSize: 16, fontWeight: 500, color: token.colorText }}>
              {t('chat.dropToAttach')}
            </span>
          </div>
        </div>
      )}

    </div>
  );
}

