import { useEffect, useMemo, useState } from 'react';
import { App as AntdApp, Button, Dropdown, Empty, Input, Select, Tooltip, theme } from 'antd';
import {
  Brain,
  ChevronDown,
  FolderOpen,
  FolderPlus,
  Home,
  ImagePlus,
  LogOut,
  MessageCircle,
  MessageSquare,
  MessageSquarePlus,
  Search,
  ScrollText,
  Settings,
  Sparkles,
  User,
  XCircle,
} from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { useUIStore } from '@/stores';
import { LoginDialog } from '@/components/LoginDialog';
import { useFrogclawAuthStore } from '@/stores/frogclawAuthStore';
import { useProviderStore } from '@/stores/providerStore';
import { useConversationStore } from '@/stores/conversationStore';
import { useSettingsStore } from '@/stores/settingsStore';
import type { Conversation, PageKey } from '@/types';
import { invoke } from '@/lib/invoke';
import { open } from '@tauri-apps/plugin-dialog';

const mainNavItems: { key: PageKey; icon: React.ReactNode; labelKey: string }[] = [
  { key: 'home', icon: <Home size={17} />, labelKey: 'nav.home' },
  { key: 'chat', icon: <MessageSquare size={17} />, labelKey: 'nav.chat' },
  { key: 'drawing', icon: <ImagePlus size={17} />, labelKey: 'nav.drawing' },
  { key: 'skills', icon: <Sparkles size={17} />, labelKey: 'nav.skills' },
  { key: 'im', icon: <MessageCircle size={17} />, labelKey: 'nav.im' },
  { key: 'logs', icon: <ScrollText size={17} />, labelKey: 'nav.logs' },
  { key: 'memory', icon: <Brain size={17} />, labelKey: 'nav.memory' },
  { key: 'files', icon: <FolderOpen size={17} />, labelKey: 'nav.files' },
];

function formatTime(timestamp: number) {
  const date = new Date(timestamp * 1000);
  const now = new Date();
  const startOfToday = new Date(now.getFullYear(), now.getMonth(), now.getDate()).getTime();
  const time = date.getTime();
  if (time >= startOfToday) {
    return date.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
  }
  return date.toLocaleDateString([], { month: '2-digit', day: '2-digit' });
}

const COLLAPSED_PROJECTS_KEY = 'frogclaw:sidebar-collapsed-projects';

function loadCollapsedProjects(): Set<string> {
  try {
    return new Set(JSON.parse(localStorage.getItem(COLLAPSED_PROJECTS_KEY) || '[]'));
  } catch {
    return new Set();
  }
}

function saveCollapsedProjects(collapsed: Set<string>) {
  localStorage.setItem(COLLAPSED_PROJECTS_KEY, JSON.stringify([...collapsed]));
}

function basename(path: string) {
  return path.replace(/[\\/]+$/, '').split(/[\\/]/).filter(Boolean).pop() || path;
}

function projectIcon() {
  return <FolderOpen size={13} />;
}

export function Sidebar() {
  const { t } = useTranslation();
  const { token } = theme.useToken();
  const { message } = AntdApp.useApp();
  const activePage = useUIStore((s) => s.activePage);
  const setActivePage = useUIStore((s) => s.setActivePage);
  const enterSettings = useUIStore((s) => s.enterSettings);
  const exitSettings = useUIStore((s) => s.exitSettings);

  const result = useFrogclawAuthStore((s) => s.result);
  const selectedTokenId = useFrogclawAuthStore((s) => s.selectedTokenId);
  const selectToken = useFrogclawAuthStore((s) => s.selectToken);
  const logout = useFrogclawAuthStore((s) => s.logout);

  const providers = useProviderStore((s) => s.providers);
  const fetchProviders = useProviderStore((s) => s.fetchProviders);
  const settings = useSettingsStore((s) => s.settings);
  const conversations = useConversationStore((s) => s.conversations);
  const activeConversationId = useConversationStore((s) => s.activeConversationId);
  const fetchConversations = useConversationStore((s) => s.fetchConversations);
  const setActiveConversation = useConversationStore((s) => s.setActiveConversation);
  const createConversation = useConversationStore((s) => s.createConversation);
  const streamingConversationId = useConversationStore((s) => s.streamingConversationId);

  const [loginOpen, setLoginOpen] = useState(false);
  const [switchingToken, setSwitchingToken] = useState(false);
  const [searchVisible, setSearchVisible] = useState(false);
  const [searchText, setSearchText] = useState('');
  const [creatingConversation, setCreatingConversation] = useState(false);
  const [creatingProject, setCreatingProject] = useState(false);
  const [defaultWorkspace, setDefaultWorkspace] = useState<string | null>(null);
  const [collapsedProjects, setCollapsedProjects] = useState<Set<string>>(() => loadCollapsedProjects());

  const user = result?.session.user ?? null;
  const tokens = result?.session.tokens ?? [];

  useEffect(() => {
    void fetchConversations();
    if (providers.length === 0) {
      void fetchProviders();
    }
    void invoke<string>('get_default_workspace_project')
      .then(setDefaultWorkspace)
      .catch(() => setDefaultWorkspace(null));
  }, [fetchConversations, fetchProviders, providers.length]);

  const displayName = user?.display_name || user?.username || t('common.login', '鐧诲綍');

  const handleSettingsToggle = () => {
    if (activePage === 'settings') {
      exitSettings();
    } else {
      enterSettings();
    }
  };

  const handleTokenChange = async (tokenId: number) => {
    setSwitchingToken(true);
    try {
      await selectToken(tokenId);
      await fetchProviders();
    } finally {
      setSwitchingToken(false);
    }
  };

  const resolveModel = () => {
    if (settings.default_provider_id && settings.default_model_id) {
      const provider = providers.find((item) => item.id === settings.default_provider_id && item.enabled);
      const model = provider?.models.find((item) => item.model_id === settings.default_model_id && item.enabled);
      if (provider && model) return { providerId: provider.id, modelId: model.model_id };
    }

    const activeConversation = conversations.find((item) => item.id === activeConversationId);
    if (activeConversation) {
      const provider = providers.find((item) => item.id === activeConversation.provider_id && item.enabled);
      const model = provider?.models.find((item) => item.model_id === activeConversation.model_id && item.enabled);
      if (provider && model) return { providerId: provider.id, modelId: model.model_id };
    }

    const provider = providers.find((item) => item.enabled && item.models.some((model) => model.enabled));
    const model = provider?.models.find((item) => item.enabled);
    return provider && model ? { providerId: provider.id, modelId: model.model_id } : null;
  };

  const handleNewConversation = async (projectPath?: string | null) => {
    const resolved = resolveModel();
    if (!resolved) {
      message.warning(t('chat.noModelsAvailable'));
      return;
    }

    let workingDirectory = projectPath ?? defaultWorkspace;
    if (!workingDirectory) {
      try {
        workingDirectory = await invoke<string>('get_default_workspace_project');
        setDefaultWorkspace(workingDirectory);
      } catch {
        workingDirectory = null;
      }
    }
    const projectName = workingDirectory ? basename(workingDirectory) : null;

    setCreatingConversation(true);
    try {
      const conversation = await createConversation(
        t('chat.newConversation'),
        resolved.modelId,
        resolved.providerId,
        { workingDirectory, projectName },
      );
      setActiveConversation(conversation.id);
      setActivePage('chat');
    } finally {
      setCreatingConversation(false);
    }
  };

  const handleSelectConversation = (conversationId: string) => {
    setActiveConversation(conversationId);
    setActivePage('chat');
  };

  const handleCreateProject = async () => {
    setCreatingProject(true);
    try {
      const selected = await open({
        directory: true,
        multiple: false,
        title: t('chat.selectProjectFolder', 'Select project folder'),
      });
      if (!selected || typeof selected !== 'string') return;
      await handleNewConversation(selected);
    } finally {
      setCreatingProject(false);
    }
  };

  const toggleProjectCollapsed = (projectPath: string) => {
    setCollapsedProjects((previous) => {
      const next = new Set(previous);
      if (next.has(projectPath)) next.delete(projectPath);
      else next.add(projectPath);
      saveCollapsedProjects(next);
      return next;
    });
  };

  const filteredConversations = useMemo(() => {
    const query = searchText.trim().toLowerCase();
    const list = query
      ? conversations.filter((item) => item.title.toLowerCase().includes(query))
      : conversations;
    return [...list].sort((left, right) => {
      if (left.is_pinned !== right.is_pinned) return left.is_pinned ? -1 : 1;
      return right.updated_at - left.updated_at;
    });
  }, [conversations, searchText]);

  const groupedConversations = useMemo(() => {
    const byProject = new Map<string, Conversation[]>();
    const defaultPath = defaultWorkspace ?? '';
    for (const conversation of filteredConversations) {
      const projectPath = conversation.working_directory || defaultPath;
      const group = byProject.get(projectPath) ?? [];
      group.push(conversation);
      byProject.set(projectPath, group);
    }

    const groups = [...byProject.entries()].map(([projectPath, groupConversations]) => {
      const sorted = [...groupConversations].sort((left, right) => right.updated_at - left.updated_at);
      return {
        key: projectPath || '__default_workspace__',
        projectPath,
        title: projectPath ? basename(projectPath) : t('chat.defaultProject', '默认项目'),
        conversations: sorted,
        latestUpdatedAt: sorted[0]?.updated_at ?? 0,
      };
    });

    if (defaultPath && (!searchText.trim() || (byProject.get(defaultPath)?.length ?? 0) > 0)) {
      if (!groups.some((group) => group.projectPath === defaultPath)) {
        groups.push({
          key: defaultPath,
          projectPath: defaultPath,
          title: basename(defaultPath),
          conversations: [],
          latestUpdatedAt: 0,
        });
      }
    }

    return groups.sort((left, right) => {
      if (left.projectPath === defaultPath) return -1;
      if (right.projectPath === defaultPath) return 1;
      return right.latestUpdatedAt - left.latestUpdatedAt;
    });
  }, [defaultWorkspace, filteredConversations, searchText, t]);
  const renderNavButton = (item: { key: PageKey; icon: React.ReactNode; labelKey: string }) => {
    const isActive = activePage === item.key;
    const label = t(item.labelKey);
    return (
      <Tooltip key={item.key} title={label} placement="right">
        <button
          onClick={() => setActivePage(item.key)}
          className="flex items-center text-base transition-colors"
          style={{
            width: '100%',
            height: 32,
            borderRadius: token.borderRadius,
            backgroundColor: isActive ? token.colorPrimaryBg : 'transparent',
            color: isActive ? token.colorPrimary : token.colorTextSecondary,
            border: 'none',
            padding: '0 10px',
            gap: 9,
            fontWeight: isActive ? 600 : 500,
            textAlign: 'left',
            cursor: 'pointer',
          }}
        >
          <span style={{ display: 'inline-flex', width: 18, justifyContent: 'center', flexShrink: 0 }}>
            {item.icon}
          </span>
          <span className="truncate" style={{ fontSize: 13 }}>
            {label}
          </span>
        </button>
      </Tooltip>
    );
  };

  const accountDropdown = user ? (
    <div style={{ width: 240, padding: 8 }}>
      <div style={{ padding: '6px 8px 10px' }}>
        <div style={{ fontSize: 13, fontWeight: 600, color: token.colorText }} className="truncate">
          {displayName}
        </div>
        <div style={{ fontSize: 12, color: token.colorTextSecondary }} className="truncate">
          {user.username}
        </div>
      </div>
      {tokens.length > 0 && (
        <div style={{ padding: '0 8px 8px' }}>
          <div style={{ fontSize: 12, color: token.colorTextSecondary, marginBottom: 6 }}>
            {t('account.apiToken', 'API 浠ょ墝')}
          </div>
          <Select
            size="small"
            style={{ width: '100%' }}
            value={selectedTokenId ?? undefined}
            disabled={switchingToken}
            options={tokens.map((frogToken) => ({
              value: frogToken.id,
              label: `${frogToken.name}${frogToken.group ? ` [${frogToken.group}]` : ''}`,
            }))}
            onChange={(id) => void handleTokenChange(id)}
          />
        </div>
      )}
      <Button
        type="text"
        danger
        block
        icon={<LogOut size={14} />}
        style={{ justifyContent: 'flex-start' }}
        onClick={logout}
      >
        {t('common.logout', 'Logout')}
      </Button>
    </div>
  ) : null;

  return (
    <div className="flex flex-col h-full" style={{ padding: '10px 10px 12px' }}>
      <div className="flex items-center gap-2" style={{ marginBottom: 8 }}>
        <Button
          type="primary"
          size="small"
          icon={<MessageSquarePlus size={15} />}
          loading={creatingConversation}
          onClick={() => void handleNewConversation()}
          style={{ flex: 1, minWidth: 0 }}
        >
          {t('chat.newConversation')}
        </Button>
        <Tooltip title={t('commandPalette.searchConversations')}>
          <Button
            size="small"
            icon={<Search size={15} />}
            onClick={() => {
              setSearchVisible((visible) => !visible);
              window.setTimeout(() => document.querySelector<HTMLInputElement>('.frogclaw-sidebar-search input')?.focus(), 40);
            }}
          />
        </Tooltip>
      </div>

      {searchVisible && (
        <Input
          className="frogclaw-sidebar-search"
          allowClear
          size="small"
          prefix={<Search size={13} />}
          placeholder={t('chat.searchPlaceholder')}
          value={searchText}
          onChange={(event) => setSearchText(event.target.value)}
          style={{ marginBottom: 8 }}
        />
      )}

      <nav className="flex flex-col gap-1" style={{ marginBottom: 10 }}>
        {mainNavItems.map(renderNavButton)}
      </nav>

      <div style={{ borderTop: `1px solid ${token.colorBorderSecondary}`, margin: '0 2px 8px' }} />

      <div className="flex items-center justify-between" style={{ padding: '0 4px 6px' }}>
        <span style={{ fontSize: 11, fontWeight: 700, color: token.colorTextTertiary, letterSpacing: 0.4 }}>
          {t('chat.conversationList', '对话列表')}
        </span>
        <Button
          type="text"
          size="small"
          icon={<FolderPlus size={13} />}
          loading={creatingProject}
          onClick={() => void handleCreateProject()}
          style={{ height: 24, paddingInline: 6, fontSize: 11, color: token.colorTextSecondary }}
        >
          {t('chat.newProject', '新建项目')}
        </Button>
      </div>

      <div className="flex-1 min-h-0 overflow-y-auto" style={{ paddingRight: 2 }}>
        {groupedConversations.length === 0 ? (
          <Empty
            image={Empty.PRESENTED_IMAGE_SIMPLE}
            description={t('chat.noConversations')}
            style={{ marginTop: 24 }}
          />
        ) : (
          <div className="flex flex-col gap-2">
            {groupedConversations.map((group) => {
              const isCollapsed = collapsedProjects.has(group.projectPath);
              return (
                <section key={group.key}>
                  <div
                    className="flex items-center"
                    style={{
                      height: 26,
                      gap: 6,
                      padding: '0 6px',
                      color: token.colorTextSecondary,
                    }}
                  >
                    <button
                      type="button"
                      onClick={() => toggleProjectCollapsed(group.projectPath)}
                      style={{
                        width: 16,
                        height: 16,
                        display: 'inline-flex',
                        alignItems: 'center',
                        justifyContent: 'center',
                        border: 'none',
                        background: 'transparent',
                        color: token.colorTextTertiary,
                        padding: 0,
                        cursor: 'pointer',
                      }}
                    >
                      <ChevronDown
                        size={13}
                        style={{
                          transform: isCollapsed ? 'rotate(-90deg)' : 'rotate(0deg)',
                          transition: 'transform 120ms ease',
                        }}
                      />
                    </button>
                    <span style={{ width: 16, display: 'inline-flex', justifyContent: 'center' }}>
                      {projectIcon()}
                    </span>
                    <span className="truncate" style={{ flex: 1, fontSize: 12, fontWeight: 650 }}>
                      {group.title}
                    </span>
                    <Tooltip title={t('chat.createInProject', '在项目中新建对话')}>
                      <Button
                        type="text"
                        size="small"
                        icon={<MessageSquarePlus size={13} />}
                        loading={creatingConversation}
                        onClick={() => void handleNewConversation(group.projectPath)}
                        style={{ width: 22, height: 22 }}
                      />
                    </Tooltip>
                  </div>

                  {!isCollapsed && (
                    <div className="flex flex-col gap-0.5">
                      {group.conversations.length === 0 ? (
                        <div style={{ padding: '6px 12px 8px 38px', color: token.colorTextTertiary, fontSize: 12 }}>
                          {t('chat.noConversations')}
                        </div>
                      ) : (
                        group.conversations.map((conversation) => {
                          const isActive = activeConversationId === conversation.id && activePage === 'chat';
                          const isStreaming = streamingConversationId === conversation.id;
                          return (
                            <button
                              key={conversation.id}
                              type="button"
                              onClick={() => handleSelectConversation(conversation.id)}
                              className="flex items-center transition-colors"
                              style={{
                                width: '100%',
                                minHeight: 34,
                                border: 'none',
                                borderRadius: token.borderRadius,
                                backgroundColor: isActive ? token.colorPrimaryBg : 'transparent',
                                color: isActive ? token.colorPrimary : token.colorText,
                                padding: '5px 8px 5px 38px',
                                gap: 8,
                                textAlign: 'left',
                                cursor: 'pointer',
                              }}
                            >
                              <MessageSquare size={14} style={{ flexShrink: 0, opacity: 0.75 }} />
                              <span className="truncate" style={{ flex: 1, fontSize: 12.5, fontWeight: isActive ? 650 : 500 }}>
                                {conversation.title || t('chat.newConversation')}
                              </span>
                              {isStreaming ? (
                                <span style={{ width: 6, height: 6, borderRadius: 999, background: token.colorPrimary, flexShrink: 0 }} />
                              ) : (
                                <span style={{ color: token.colorTextTertiary, fontSize: 11, flexShrink: 0 }}>
                                  {formatTime(conversation.updated_at)}
                                </span>
                              )}
                            </button>
                          );
                        })
                      )}
                    </div>
                  )}
                </section>
              );
            })}
          </div>
        )}
      </div>

      <div style={{ borderTop: `1px solid ${token.colorBorderSecondary}`, margin: '8px 2px' }} />

      {user ? (
        <Dropdown dropdownRender={() => accountDropdown} trigger={['click']} placement="topLeft">
          <button
            className="flex items-center transition-colors"
            style={{
              width: '100%',
              height: 36,
              borderRadius: token.borderRadius,
              background: 'transparent',
              border: 'none',
              padding: '0 10px',
              gap: 9,
              color: token.colorTextSecondary,
              cursor: 'pointer',
              textAlign: 'left',
            }}
          >
            <span style={{ display: 'inline-flex', width: 18, justifyContent: 'center', flexShrink: 0 }}>
              <User size={17} />
            </span>
            <span className="truncate" style={{ fontSize: 13, fontWeight: 500, flex: 1 }}>{displayName}</span>
          </button>
        </Dropdown>
      ) : (
        <Tooltip title={t('account.loginFrogClaw', '鐧诲綍 FrogClaw')} placement="right">
          <button
            onClick={() => setLoginOpen(true)}
            className="flex items-center transition-colors"
            style={{
              width: '100%',
              height: 36,
              borderRadius: token.borderRadius,
              background: 'transparent',
              border: 'none',
              padding: '0 10px',
              gap: 9,
              color: token.colorTextSecondary,
              cursor: 'pointer',
              textAlign: 'left',
            }}
          >
            <span style={{ display: 'inline-flex', width: 18, justifyContent: 'center', flexShrink: 0 }}>
              <User size={17} />
            </span>
            <span className="truncate" style={{ fontSize: 13, fontWeight: 500 }}>鐧诲綍</span>
          </button>
        </Tooltip>
      )}

      <Tooltip
        title={activePage === 'settings' ? t('settings.closeSettings') : t('settings.openSettings')}
        placement="right"
      >
        <button
          onClick={handleSettingsToggle}
          className="flex items-center transition-colors"
          style={{
            width: '100%',
            height: 34,
            borderRadius: token.borderRadius,
            backgroundColor: activePage === 'settings' ? token.colorPrimaryBg : 'transparent',
            color: activePage === 'settings' ? token.colorPrimary : token.colorTextSecondary,
            border: 'none',
            padding: '0 10px',
            gap: 9,
            fontWeight: activePage === 'settings' ? 600 : 500,
            textAlign: 'left',
            cursor: 'pointer',
          }}
        >
          <span style={{ display: 'inline-flex', width: 18, justifyContent: 'center', flexShrink: 0 }}>
            {activePage === 'settings' ? <XCircle size={17} /> : <Settings size={17} />}
          </span>
          <span className="truncate" style={{ fontSize: 13 }}>
            {activePage === 'settings' ? t('settings.closeSettings') : t('settings.openSettings')}
          </span>
        </button>
      </Tooltip>

      <LoginDialog open={loginOpen} onOpenChange={setLoginOpen} />
    </div>
  );
}
