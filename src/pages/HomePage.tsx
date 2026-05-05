import { useCallback, useEffect, useMemo, useState } from 'react';
import { App, Button, Card, Form, Input, List, Progress, Select, Space, Tag, Typography, theme } from 'antd';
import { getVersion } from '@tauri-apps/api/app';
import {
  Check,
  CheckCircle2,
  Download,
  ExternalLink,
  KeyRound,
  Loader2,
  LogIn,
  MessageSquare,
  RefreshCw,
  Terminal,
  UserRound,
  XCircle,
} from 'lucide-react';
import { invoke } from '@tauri-apps/api/core';
import { checkToolsInstalled, installTool } from '@/lib/homeApi';
import { FROGCLAW_BASE_URL } from '@/lib/frogclawConfig';
import { useFrogclawAuthStore } from '@/stores/frogclawAuthStore';
import { useProviderStore } from '@/stores/providerStore';
import { useUIStore } from '@/stores';
import type { ToolStatus } from '@/types';

const { Text, Title, Paragraph } = Typography;

const INSTALL_ORDER = ['node', 'git', 'claude', 'codex', 'gemini'];

type ImChannel = {
  id: string;
  platform: 'feishu' | 'qq';
  enabled: boolean;
  assignment?: 'aiagent' | 'native_cli' | 'frogclaw' | 'none' | null;
};

type PlatformStatus = {
  running: boolean;
};

function toolStatusText(tool: ToolStatus) {
  if (!tool.installed) return '未安装';
  if (tool.needs_upgrade) return '需要升级';
  return tool.version?.split('\n')[0] || '已安装';
}

function maskTokenKey(key: string) {
  if (!key) return '';
  if (key.length <= 8) return 'sk-****';
  return `sk-${key.slice(0, 4)}...${key.slice(-4)}`;
}

function SetupCard({
  step,
  title,
  description,
  icon,
  completed,
  headerRight,
  children,
}: {
  step: number;
  title: string;
  description: React.ReactNode;
  icon: React.ReactNode;
  completed?: boolean;
  headerRight?: React.ReactNode;
  children: React.ReactNode;
}) {
  const { token } = theme.useToken();
  return (
    <Card
      styles={{ body: { padding: 20 } }}
      style={{
        borderColor: completed ? token.colorSuccessBorder : token.colorBorderSecondary,
        borderRadius: 12,
        boxShadow: token.boxShadowTertiary,
      }}
    >
      <div style={{ display: 'flex', justifyContent: 'space-between', gap: 12, marginBottom: 16 }}>
        <div style={{ display: 'flex', gap: 12, minWidth: 0 }}>
          <div
            style={{
              width: 24,
              height: 24,
              borderRadius: '50%',
              background: completed ? token.colorSuccess : token.colorPrimary,
              color: '#fff',
              display: 'flex',
              alignItems: 'center',
              justifyContent: 'center',
              fontSize: 12,
              fontWeight: 700,
              flexShrink: 0,
            }}
          >
            {completed ? <Check size={14} /> : step}
          </div>
          <div
            style={{
              width: 40,
              height: 40,
              borderRadius: 8,
              border: `1px solid ${token.colorBorderSecondary}`,
              background: token.colorBgContainer,
              display: 'flex',
              alignItems: 'center',
              justifyContent: 'center',
              color: token.colorPrimary,
              flexShrink: 0,
            }}
          >
            {icon}
          </div>
          <div style={{ minWidth: 0 }}>
            <Title level={5} style={{ margin: 0 }}>{title}</Title>
            <Text type="secondary" style={{ fontSize: 12 }}>{description}</Text>
          </div>
        </div>
        {headerRight}
      </div>
      {children}
    </Card>
  );
}

function DevEnvironmentCard({ step }: { step: number }) {
  const { message } = App.useApp();
  const { token } = theme.useToken();
  const [tools, setTools] = useState<ToolStatus[]>([]);
  const [loading, setLoading] = useState(false);
  const [installingId, setInstallingId] = useState<string | null>(null);
  const [installingAll, setInstallingAll] = useState(false);

  const loadTools = useCallback(async () => {
    setLoading(true);
    try {
      const status = await checkToolsInstalled();
      setTools(status.tools);
    } catch (error) {
      message.error(`检测工具失败：${String(error)}`);
    } finally {
      setLoading(false);
    }
  }, [message]);

  useEffect(() => {
    void loadTools();
  }, [loadTools]);

  const installedCount = tools.filter((tool) => tool.installed && !tool.needs_upgrade).length;
  const missingTools = useMemo(
    () =>
      INSTALL_ORDER
        .map((id) => tools.find((tool) => tool.id === id))
        .filter((tool): tool is ToolStatus => !!tool && (!tool.installed || tool.needs_upgrade) && tool.installable),
    [tools],
  );
  const toolsReady = tools.length > 0 && installedCount === tools.length;

  const handleInstall = async (tool: ToolStatus) => {
    setInstallingId(tool.id);
    try {
      const installResult = await installTool(tool.id);
      if (installResult.success) message.success(installResult.message);
      else message.error(installResult.log_file ? `${installResult.message}，日志：${installResult.log_file}` : installResult.message);
      await loadTools();
    } catch (error) {
      message.error(`${tool.name} 安装失败：${String(error)}`);
    } finally {
      setInstallingId(null);
    }
  };

  const handleInstallAll = async () => {
    setInstallingAll(true);
    try {
      for (const tool of missingTools) {
        await handleInstall(tool);
      }
    } finally {
      setInstallingAll(false);
    }
  };

  return (
    <SetupCard
      step={step}
      title="开发环境检测"
      description="检测并一键安装 Node.js、Git、Claude Code、Codex 和 Gemini CLI"
      icon={<Terminal size={20} />}
      completed={toolsReady}
      headerRight={
        <Space>
          {!loading && tools.length > 0 && (
            <Tag color={toolsReady ? 'success' : 'warning'}>{installedCount}/{tools.length}</Tag>
          )}
          <Button
            size="small"
            icon={loading ? <Loader2 size={14} className="animate-spin" /> : <RefreshCw size={14} />}
            onClick={loadTools}
            disabled={loading || installingAll || !!installingId}
          />
        </Space>
      }
    >
      {missingTools.length > 0 && !loading && (
        <div
          style={{
            display: 'flex',
            justifyContent: 'space-between',
            alignItems: 'center',
            gap: 12,
            border: `1px solid ${token.colorWarningBorder}`,
            background: token.colorWarningBg,
            borderRadius: 8,
            padding: '8px 12px',
            marginBottom: 12,
          }}
        >
          <Text style={{ fontSize: 12, color: token.colorWarningText }}>有 {missingTools.length} 个工具未安装或需要升级</Text>
          <Button
            type="primary"
            size="small"
            icon={installingAll ? <Loader2 size={13} className="animate-spin" /> : <Download size={13} />}
            onClick={handleInstallAll}
            disabled={installingAll || !!installingId}
          >
            一键安装
          </Button>
        </div>
      )}

      <List
        grid={{ gutter: 8, xs: 1, sm: 2, md: 3 }}
        dataSource={tools}
        loading={loading && tools.length === 0}
        renderItem={(tool) => {
          const ready = tool.installed && !tool.needs_upgrade;
          return (
            <List.Item>
              <div
                style={{
                  border: `1px solid ${ready ? token.colorSuccessBorder : token.colorBorderSecondary}`,
                  borderRadius: 8,
                  padding: 10,
                  minHeight: 72,
                  display: 'flex',
                  alignItems: 'center',
                  gap: 10,
                  background: ready ? token.colorSuccessBg : token.colorBgContainer,
                }}
              >
                {ready ? <CheckCircle2 size={16} color={token.colorSuccess} /> : <XCircle size={16} color={tool.needs_upgrade ? token.colorWarning : token.colorError} />}
                <div style={{ minWidth: 0, flex: 1 }}>
                  <div style={{ fontWeight: 600, fontSize: 13 }}>{tool.name}</div>
                  <Text type="secondary" ellipsis style={{ maxWidth: 160, fontSize: 12 }}>{toolStatusText(tool)}</Text>
                </div>
                {!ready && tool.installable ? (
                  <Button
                    size="small"
                    onClick={() => void handleInstall(tool)}
                    disabled={installingAll || !!installingId}
                    icon={installingId === tool.id ? <Loader2 size={12} className="animate-spin" /> : <Download size={12} />}
                  >
                    {tool.needs_upgrade ? '升级' : '安装'}
                  </Button>
                ) : <Tag color="success">OK</Tag>}
              </div>
            </List.Item>
          );
        }}
      />
      {tools.length > 0 && (
        <Progress
          percent={Math.round((installedCount / tools.length) * 100)}
          size="small"
          status={toolsReady ? 'success' : 'active'}
          style={{ marginTop: 8 }}
        />
      )}
    </SetupCard>
  );
}

function FrogclawCard({ step }: { step: number }) {
  const { message } = App.useApp();
  const { token } = theme.useToken();
  const [form] = Form.useForm<{ username: string; password: string }>();
  const result = useFrogclawAuthStore((s) => s.result);
  const selectedTokenId = useFrogclawAuthStore((s) => s.selectedTokenId);
  const login = useFrogclawAuthStore((s) => s.login);
  const logout = useFrogclawAuthStore((s) => s.logout);
  const selectToken = useFrogclawAuthStore((s) => s.selectToken);
  const fetchProviders = useProviderStore((s) => s.fetchProviders);
  const [loading, setLoading] = useState(false);
  const [tokenApplying, setTokenApplying] = useState(false);

  const user = result?.session.user ?? null;
  const tokens = result?.session.tokens ?? [];

  const handleLogin = async (values: { username: string; password: string }) => {
    setLoading(true);
    try {
      await login(values.username, values.password);
      await fetchProviders();
      form.resetFields();
      message.success('已登录 FrogClaw，并同步可用模型与供应商令牌');
    } catch (error) {
      message.error(`登录或配置失败：${String(error)}`);
    } finally {
      setLoading(false);
    }
  };

  const handleTokenChange = async (tokenId: number) => {
    setTokenApplying(true);
    try {
      await selectToken(tokenId);
      await fetchProviders();
      message.success('已切换令牌，并同步更新供应商密钥');
    } catch (error) {
      message.error(`切换令牌失败：${String(error)}`);
    } finally {
      setTokenApplying(false);
    }
  };

  return (
    <SetupCard
      step={step}
      title="FrogClaw 连接"
      description={(
        <span>
          登录 {FROGCLAW_BASE_URL} 并配置 API 令牌。
          {' '}
          <Typography.Link href="https://frogclaw.com/console/token" target="_blank">
            创建令牌 <ExternalLink size={12} style={{ verticalAlign: -2 }} />
          </Typography.Link>
        </span>
      )}
      icon={<KeyRound size={20} />}
      completed={!!user}
      headerRight={user ? <Tag color="success">已连接</Tag> : undefined}
    >
      {!user ? (
        <Form form={form} layout="vertical" onFinish={handleLogin}>
          <Form.Item name="username" label="用户名" rules={[{ required: true, message: '请输入用户名' }]}>
            <Input prefix={<UserRound size={14} />} placeholder="请输入用户名" disabled={loading} />
          </Form.Item>
          <Form.Item name="password" label="密码" rules={[{ required: true, message: '请输入密码' }]}>
            <Input.Password placeholder="请输入密码" disabled={loading} />
          </Form.Item>
          <Button type="primary" htmlType="submit" block loading={loading} icon={!loading ? <LogIn size={14} /> : undefined}>
            登录
          </Button>
        </Form>
      ) : (
        <Space direction="vertical" size={12} style={{ width: '100%' }}>
          <div
            style={{
              display: 'flex',
              justifyContent: 'space-between',
              gap: 12,
              alignItems: 'center',
              border: `1px solid ${token.colorBorderSecondary}`,
              borderRadius: 8,
              background: token.colorFillAlter,
              padding: '10px 12px',
            }}
          >
            <div style={{ minWidth: 0 }}>
              <Text type="secondary" style={{ fontSize: 12 }}>当前用户</Text>
              <div className="truncate" style={{ fontSize: 14, fontWeight: 600 }}>{user.display_name || user.username}</div>
            </div>
            <Button size="small" type="text" danger onClick={logout}>退出</Button>
          </div>

          <div>
            <Text strong style={{ fontSize: 13 }}>用户令牌</Text>
            <Select
              value={selectedTokenId ?? result?.selected_token_id ?? undefined}
              loading={tokenApplying}
              disabled={tokenApplying || tokens.length === 0}
              style={{ width: '100%', marginTop: 8 }}
              placeholder="选择要写入供应商的令牌"
              onChange={(id) => void handleTokenChange(id)}
              options={tokens.map((frogToken) => ({
                value: frogToken.id,
                label: `${frogToken.name} (${frogToken.group || 'default'} / ${maskTokenKey(frogToken.key)})`,
              }))}
            />
            <div style={{ display: 'flex', flexWrap: 'wrap', gap: 8, marginTop: 8 }}>
              {tokens.map((frogToken) => (
                <Tag key={frogToken.id} color={frogToken.id === selectedTokenId ? 'blue' : undefined}>
                  {frogToken.name}
                  <Text type="secondary" style={{ marginLeft: 6, fontSize: 11 }}>
                    {frogToken.group || 'default'} / {maskTokenKey(frogToken.key)}
                  </Text>
                </Tag>
              ))}
            </div>
          </div>

          {result && result.configured_providers.length > 0 && (
            <div>
              <Text strong style={{ fontSize: 13 }}>已写入供应商</Text>
              <div style={{ display: 'flex', flexWrap: 'wrap', gap: 8, marginTop: 8 }}>
                {result.configured_providers.map((provider) => (
                  <Tag key={provider.provider_id} color={provider.updated_key ? 'cyan' : undefined}>
                    {provider.name} · {provider.model_count} 模型
                  </Tag>
                ))}
              </div>
            </div>
          )}
        </Space>
      )}
    </SetupCard>
  );
}

const FeishuIcon = ({ size = 22 }: { size?: number }) => (
  <svg viewBox="0 0 48 48" width={size} height={size} xmlns="http://www.w3.org/2000/svg">
    <path fill="#00D6B9" d="M32.5 12C38 12 42 16 42 21.5V36c0 .5-.6.8-1 .5-6.3-4.8-11.3-8-17-8-3.2 0-6 .7-9 2.2-.4.2-.8-.1-.8-.5V21.5C14.2 16 18.2 12 23.7 12h8.8z" />
    <path fill="#3370FF" d="M6 22.8c0-.5.6-.8 1-.5 4.8 3.6 9.3 7 14.5 9.7 4.8 2.4 9.5 3 14.7 2.5.5 0 .8.4.6.8-2.5 4.3-7.2 7.2-12.5 7.2-3 0-5.8-.9-8.2-2.4C10.2 37 6 31.4 6 24.8v-2z" />
  </svg>
);

const QQIcon = ({ size = 22 }: { size?: number }) => (
  <svg viewBox="0 0 48 48" width={size} height={size} fill="none" xmlns="http://www.w3.org/2000/svg">
    <path fill="#12B7F5" d="M24 4C15.2 4 10 10.5 10 18.5c0 2.6.5 5 1.4 7-.9 1.4-2.4 4-2.4 5.5 0 1 .6 1.2 1.4.6l2.3-1.9c1 1 2.4 2 4 2.7-.5 1-1 2-1 3 0 2.4 1.8 3.6 5.5 3.6h5.6c3.7 0 5.5-1.2 5.5-3.6 0-1-.5-2-1-3 1.6-.7 3-1.7 4-2.7l2.3 1.9c.8.6 1.4.4 1.4-.6 0-1.5-1.5-4.1-2.4-5.5.9-2 1.4-4.4 1.4-7C38 10.5 32.8 4 24 4z" />
    <circle fill="white" cx="18" cy="18" r="3" />
    <circle fill="white" cx="30" cy="18" r="3" />
    <circle fill="#12B7F5" cx="18" cy="18" r="1.5" />
    <circle fill="#12B7F5" cx="30" cy="18" r="1.5" />
  </svg>
);

function ChannelQuickCard({
  icon,
  name,
  description,
  statusColor,
  statusText,
  onClick,
}: {
  icon: React.ReactNode;
  name: string;
  description: string;
  statusColor: string;
  statusText: string;
  onClick: () => void;
}) {
  const { token } = theme.useToken();
  return (
    <div
      style={{
        border: `1px solid ${token.colorBorderSecondary}`,
        borderRadius: 8,
        background: token.colorBgContainer,
        padding: 12,
        display: 'flex',
        flexDirection: 'column',
        gap: 10,
      }}
    >
      <div style={{ display: 'flex', gap: 10, alignItems: 'center' }}>
        <div
          style={{
            width: 36,
            height: 36,
            borderRadius: 8,
            border: `1px solid ${token.colorBorderSecondary}`,
            display: 'flex',
            alignItems: 'center',
            justifyContent: 'center',
            background: '#fff',
            flexShrink: 0,
          }}
        >
          {icon}
        </div>
        <div style={{ minWidth: 0, flex: 1 }}>
          <div style={{ fontSize: 14, fontWeight: 600 }}>{name}</div>
          <div style={{ display: 'flex', gap: 6, alignItems: 'center' }}>
            <span style={{ width: 7, height: 7, borderRadius: '50%', background: statusColor }} />
            <Text type="secondary" style={{ fontSize: 11 }}>{statusText}</Text>
          </div>
        </div>
      </div>
      <Text type="secondary" style={{ fontSize: 12, minHeight: 36 }}>{description}</Text>
      <Button size="small" icon={<ExternalLink size={13} />} onClick={onClick}>配置</Button>
    </div>
  );
}

function IMChannelCard({ step }: { step: number }) {
  const setActivePage = useUIStore((s) => s.setActivePage);
  const [channels, setChannels] = useState<ImChannel[]>([]);
  const [status, setStatus] = useState<PlatformStatus | null>(null);

  const load = useCallback(async () => {
    const [remoteChannels, remoteStatus] = await Promise.all([
      invoke<ImChannel[]>('get_im_channels').catch(() => []),
      invoke<PlatformStatus>('platform_status').catch(() => ({ running: false })),
    ]);
    setChannels(remoteChannels);
    setStatus(remoteStatus);
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  const hasFeishu = channels.some((ch) => ch.platform === 'feishu' && ch.enabled);
  const hasQQ = channels.some((ch) => ch.platform === 'qq' && ch.enabled);
  const running = Boolean(status?.running);
  const goToChannels = () => setActivePage('im');

  return (
    <SetupCard
      step={step}
      title="IM 通道设置"
      description="配置飞书和 QQ 机器人，让消息进入 AI Agent 或原生 CLI"
      icon={<MessageSquare size={20} />}
      completed={running && (hasFeishu || hasQQ)}
    >
      <div
        style={{
          border: '1px solid rgba(59, 130, 246, 0.22)',
          background: 'rgba(59, 130, 246, 0.06)',
          borderRadius: 8,
          padding: '8px 12px',
          marginBottom: 12,
        }}
      >
        <Text style={{ fontSize: 12, color: '#2563eb' }}>
          Sidecar 默认随应用启动。添加飞书或 QQ 通道并选择 AI Agent 或原生 CLI 后，机器人会自动连接。
        </Text>
      </div>
      <div style={{ display: 'grid', gridTemplateColumns: 'repeat(auto-fit, minmax(220px, 1fr))', gap: 10 }}>
        <ChannelQuickCard
          icon={<FeishuIcon />}
          name="飞书"
          description="通过飞书机器人接收消息，支持私聊和群聊 @ 触发。"
          statusColor={running && hasFeishu ? '#22c55e' : '#94a3b8'}
          statusText={running && hasFeishu ? '已配置' : '未配置'}
          onClick={goToChannels}
        />
        <ChannelQuickCard
          icon={<QQIcon />}
          name="QQ"
          description="通过 QQ 官方机器人接入群聊和 C2C 会话。"
          statusColor={running && hasQQ ? '#22c55e' : '#94a3b8'}
          statusText={running && hasQQ ? '已配置' : '未配置'}
          onClick={goToChannels}
        />
      </div>
    </SetupCard>
  );
}

export function HomePage() {
  const { token } = theme.useToken();
  const [appVersion, setAppVersion] = useState('');

  useEffect(() => {
    getVersion().then(setAppVersion).catch(() => {});
  }, []);

  return (
    <div style={{ height: '100%', overflow: 'auto', background: token.colorBgLayout }}>
      <div style={{ maxWidth: 1080, margin: '0 auto', padding: 24 }}>
        <div style={{ marginBottom: 20 }}>
          <Title level={2} style={{ margin: 0 }}>
            FrogClaw 首页
            {appVersion && <Text type="secondary" style={{ marginLeft: 12, fontSize: 14, fontWeight: 400 }}>v{appVersion}</Text>}
          </Title>
          <Paragraph type="secondary" style={{ marginTop: 8, marginBottom: 0 }}>
            检测本机 CLI 环境，登录 {FROGCLAW_BASE_URL} 获取令牌，并把可用供应商写入 FrogClawClient 配置。
          </Paragraph>
        </div>

        <Space direction="vertical" size={16} style={{ width: '100%' }}>
          <DevEnvironmentCard step={1} />
          <FrogclawCard step={2} />
          <IMChannelCard step={3} />
        </Space>
      </div>
    </div>
  );
}
