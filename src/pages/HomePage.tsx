import { useCallback, useEffect, useMemo, useState } from 'react';
import { App, Button, Card, Descriptions, Form, Input, List, Progress, Space, Tag, Typography, theme } from 'antd';
import { CheckCircle2, Download, KeyRound, Loader2, LogIn, RefreshCw, Terminal, UserRound, XCircle } from 'lucide-react';
import { getVersion } from '@tauri-apps/api/app';
import { checkToolsInstalled, fetchAndConfigureFrogclaw, installTool } from '@/lib/homeApi';
import { useProviderStore } from '@/stores/providerStore';
import type { FrogclawConfigureResult, ToolStatus } from '@/types';

const { Text, Title, Paragraph } = Typography;

const INSTALL_ORDER = ['node', 'git', 'claude', 'codex', 'gemini', 'openclaw'];

function statusColor(tool: ToolStatus) {
  if (!tool.installed) return 'error';
  if (tool.needs_upgrade) return 'warning';
  return 'success';
}

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
  children,
}: {
  step: number;
  title: string;
  description: string;
  icon: React.ReactNode;
  completed?: boolean;
  children: React.ReactNode;
}) {
  const { token } = theme.useToken();
  return (
    <Card
      styles={{ body: { padding: 20 } }}
      style={{
        borderColor: completed ? token.colorSuccessBorder : token.colorBorderSecondary,
        borderRadius: token.borderRadius,
      }}
    >
      <div style={{ display: 'flex', gap: 12, alignItems: 'flex-start', marginBottom: 16 }}>
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
          {completed ? <CheckCircle2 size={14} /> : step}
        </div>
        <div
          style={{
            width: 40,
            height: 40,
            borderRadius: token.borderRadius,
            border: `1px solid ${token.colorBorderSecondary}`,
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
          <Title level={5} style={{ margin: 0 }}>
            {title}
          </Title>
          <Text type="secondary" style={{ fontSize: 12 }}>
            {description}
          </Text>
        </div>
      </div>
      {children}
    </Card>
  );
}

export function HomePage() {
  const { message } = App.useApp();
  const { token } = theme.useToken();
  const fetchProviders = useProviderStore((s) => s.fetchProviders);
  const [appVersion, setAppVersion] = useState('');
  const [tools, setTools] = useState<ToolStatus[]>([]);
  const [toolsLoading, setToolsLoading] = useState(false);
  const [installingId, setInstallingId] = useState<string | null>(null);
  const [installingAll, setInstallingAll] = useState(false);
  const [loginLoading, setLoginLoading] = useState(false);
  const [result, setResult] = useState<FrogclawConfigureResult | null>(() => {
    const raw = localStorage.getItem('frogclaw_home_last_result');
    if (!raw) return null;
    try {
      return JSON.parse(raw) as FrogclawConfigureResult;
    } catch {
      return null;
    }
  });

  const loadTools = useCallback(async () => {
    setToolsLoading(true);
    try {
      const status = await checkToolsInstalled();
      setTools(status.tools);
    } catch (error) {
      message.error(`检测工具失败: ${String(error)}`);
    } finally {
      setToolsLoading(false);
    }
  }, [message]);

  useEffect(() => {
    getVersion().then(setAppVersion).catch(() => {});
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

  const handleInstall = useCallback(
    async (tool: ToolStatus) => {
      setInstallingId(tool.id);
      try {
        const installResult = await installTool(tool.id);
        if (installResult.success) {
          message.success(installResult.message);
        } else {
          const suffix = installResult.log_file ? `，日志: ${installResult.log_file}` : '';
          message.error(`${installResult.message}${suffix}`);
        }
        await loadTools();
      } catch (error) {
        message.error(`${tool.name} 安装失败: ${String(error)}`);
      } finally {
        setInstallingId(null);
      }
    },
    [loadTools, message],
  );

  const handleInstallAll = useCallback(async () => {
    setInstallingAll(true);
    try {
      for (const tool of missingTools) {
        setInstallingId(tool.id);
        const installResult = await installTool(tool.id);
        if (!installResult.success) {
          const suffix = installResult.log_file ? `，日志: ${installResult.log_file}` : '';
          message.error(`${tool.name}: ${installResult.message}${suffix}`);
        } else {
          message.success(`${tool.name}: ${installResult.message}`);
        }
        await loadTools();
      }
    } finally {
      setInstallingId(null);
      setInstallingAll(false);
    }
  }, [loadTools, message, missingTools]);

  const handleLogin = useCallback(
    async (values: { username: string; password: string }) => {
      setLoginLoading(true);
      try {
        const configureResult = await fetchAndConfigureFrogclaw(values.username, values.password);
        setResult(configureResult);
        localStorage.setItem('frogclaw_home_last_result', JSON.stringify(configureResult));
        await fetchProviders();
        message.success('已登录 FrogClaw，并完成可用模型供应商配置');
      } catch (error) {
        message.error(`登录或配置失败: ${String(error)}`);
      } finally {
        setLoginLoading(false);
      }
    },
    [fetchProviders, message],
  );

  return (
    <div style={{ height: '100%', overflow: 'auto', background: token.colorBgLayout }}>
      <div style={{ maxWidth: 1080, margin: '0 auto', padding: 24 }}>
        <div style={{ marginBottom: 20 }}>
          <Title level={2} style={{ margin: 0 }}>
            FrogClaw 首页
            {appVersion && (
              <Text type="secondary" style={{ marginLeft: 12, fontSize: 14, fontWeight: 400 }}>
                v{appVersion}
              </Text>
            )}
          </Title>
          <Paragraph type="secondary" style={{ marginTop: 8, marginBottom: 0 }}>
            检测本机 CLI 环境，登录 frogclaw.com 获取令牌，并把可用供应商直接写入 FrogClawClient 配置。
          </Paragraph>
        </div>

        <Space direction="vertical" size={16} style={{ width: '100%' }}>
          <SetupCard
            step={1}
            title="开发环境"
            description="检测并一键安装 Node.js、Git、Claude Code、Codex、Gemini CLI 和 OpenClaw"
            icon={<Terminal size={20} />}
            completed={toolsReady}
          >
            <div style={{ display: 'flex', justifyContent: 'space-between', gap: 12, alignItems: 'center', marginBottom: 12 }}>
              <div style={{ flex: 1 }}>
                <Progress
                  percent={tools.length ? Math.round((installedCount / tools.length) * 100) : 0}
                  size="small"
                  status={toolsReady ? 'success' : 'active'}
                />
              </div>
              <Space>
                <Button
                  icon={toolsLoading ? <Loader2 size={14} className="animate-spin" /> : <RefreshCw size={14} />}
                  onClick={loadTools}
                  disabled={toolsLoading || installingAll || !!installingId}
                >
                  刷新
                </Button>
                <Button
                  type="primary"
                  icon={installingAll ? <Loader2 size={14} className="animate-spin" /> : <Download size={14} />}
                  disabled={missingTools.length === 0 || installingAll || !!installingId}
                  onClick={handleInstallAll}
                >
                  一键安装
                </Button>
              </Space>
            </div>

            <List
              grid={{ gutter: 8, xs: 1, sm: 2, md: 3 }}
              dataSource={tools}
              loading={toolsLoading && tools.length === 0}
              renderItem={(tool) => (
                <List.Item>
                  <div
                    style={{
                      border: `1px solid ${token.colorBorderSecondary}`,
                      borderRadius: token.borderRadius,
                      padding: 10,
                      minHeight: 72,
                      display: 'flex',
                      alignItems: 'center',
                      gap: 10,
                      background: token.colorBgContainer,
                    }}
                  >
                    {tool.installed && !tool.needs_upgrade ? (
                      <CheckCircle2 size={16} color={token.colorSuccess} />
                    ) : (
                      <XCircle size={16} color={tool.needs_upgrade ? token.colorWarning : token.colorError} />
                    )}
                    <div style={{ minWidth: 0, flex: 1 }}>
                      <div style={{ fontWeight: 600, fontSize: 13 }}>{tool.name}</div>
                      <Text type="secondary" ellipsis style={{ maxWidth: 160, fontSize: 12 }}>
                        {toolStatusText(tool)}
                      </Text>
                    </div>
                    {(!tool.installed || tool.needs_upgrade) && (
                      <Button
                        size="small"
                        onClick={() => handleInstall(tool)}
                        disabled={installingAll || !!installingId}
                        icon={installingId === tool.id ? <Loader2 size={12} className="animate-spin" /> : <Download size={12} />}
                      >
                        {tool.needs_upgrade ? '升级' : '安装'}
                      </Button>
                    )}
                    {tool.installed && !tool.needs_upgrade && <Tag color={statusColor(tool)}>OK</Tag>}
                  </div>
                </List.Item>
              )}
            />
          </SetupCard>

          <SetupCard
            step={2}
            title="FrogClaw 登录与令牌配置"
            description="登录 frogclaw.com 后自动获取可用令牌、供应商和 OpenClaw 配置"
            icon={<KeyRound size={20} />}
            completed={!!result?.configured_providers.length}
          >
            <Form layout="inline" onFinish={handleLogin} style={{ marginBottom: result ? 16 : 0 }}>
              <Form.Item name="username" rules={[{ required: true, message: '请输入用户名' }]} style={{ minWidth: 220 }}>
                <Input prefix={<UserRound size={14} />} placeholder="用户名" disabled={loginLoading} />
              </Form.Item>
              <Form.Item name="password" rules={[{ required: true, message: '请输入密码' }]} style={{ minWidth: 220 }}>
                <Input.Password placeholder="密码" disabled={loginLoading} />
              </Form.Item>
              <Form.Item>
                <Button
                  type="primary"
                  htmlType="submit"
                  loading={loginLoading}
                  icon={!loginLoading ? <LogIn size={14} /> : undefined}
                >
                  登录并配置
                </Button>
              </Form.Item>
            </Form>

            {result && (
              <Space direction="vertical" size={12} style={{ width: '100%' }}>
                <Descriptions size="small" column={3} bordered>
                  <Descriptions.Item label="当前用户">
                    {result.session.user.display_name || result.session.user.username}
                  </Descriptions.Item>
                  <Descriptions.Item label="可用令牌">{result.session.tokens.length}</Descriptions.Item>
                  <Descriptions.Item label="已配置供应商">{result.configured_providers.length}</Descriptions.Item>
                </Descriptions>

                {result.session.tokens.length > 0 && (
                  <div>
                    <Text strong>令牌</Text>
                    <div style={{ display: 'flex', flexWrap: 'wrap', gap: 8, marginTop: 8 }}>
                      {result.session.tokens.map((frogToken) => (
                        <Tag key={frogToken.id}>
                          {frogToken.name}
                          <Text type="secondary" style={{ marginLeft: 6, fontSize: 11 }}>
                            {frogToken.group || 'default'} / {maskTokenKey(frogToken.key)}
                          </Text>
                        </Tag>
                      ))}
                    </div>
                  </div>
                )}

                {result.configured_providers.length > 0 && (
                  <List
                    size="small"
                    header={<Text strong>已写入 FrogClawClient 的供应商</Text>}
                    bordered
                    dataSource={result.configured_providers}
                    renderItem={(provider) => (
                      <List.Item>
                        <Space wrap>
                          <Text strong>{provider.name}</Text>
                          <Tag>{provider.provider_type}</Tag>
                          {provider.model_id && <Tag color="blue">{provider.model_id}</Tag>}
                          <Text type="secondary">
                            {provider.token_name} ({provider.token_group})
                          </Text>
                          {provider.created_provider ? <Tag color="green">新建</Tag> : <Tag>复用</Tag>}
                          {provider.added_key && <Tag color="cyan">已添加密钥</Tag>}
                          {provider.reused_key && <Tag>密钥已存在</Tag>}
                        </Space>
                      </List.Item>
                    )}
                  />
                )}

                <div>
                  <Text strong>OpenClaw 配置</Text>
                  <div style={{ marginTop: 8 }}>
                    {result.openclaw.applied ? (
                      <Tag color="green">已写入 {result.openclaw.path}</Tag>
                    ) : (
                      <Tag>未发现服务端 OpenClaw 配置</Tag>
                    )}
                    {result.openclaw.models.length > 0 && (
                      <div style={{ display: 'flex', flexWrap: 'wrap', gap: 8, marginTop: 8 }}>
                        {result.openclaw.models.map((model) => (
                          <Tag key={`${model.provider}/${model.id}`} color="purple">
                            {model.name}
                          </Tag>
                        ))}
                      </div>
                    )}
                  </div>
                </div>
              </Space>
            )}
          </SetupCard>
        </Space>
      </div>
    </div>
  );
}
