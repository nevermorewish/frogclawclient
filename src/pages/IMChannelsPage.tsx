import { useEffect, useMemo, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { openUrl } from '@tauri-apps/plugin-opener';
import {
  App,
  Button,
  Card,
  Empty,
  Form,
  Input,
  Modal,
  Select,
  Space,
  Switch,
  Table,
  Tag,
  Tooltip,
  Typography,
  theme,
} from 'antd';
import type { ColumnsType } from 'antd/es/table';
import {
  Bot,
  CircleOff,
  Edit3,
  ExternalLink,
  MessageCircle,
  Plus,
  RefreshCw,
  Save,
  Trash2,
} from 'lucide-react';

type ImPlatform = 'feishu' | 'qq';
type Assignment = 'aiagent' | 'native_cli';

type ImChannel = {
  id: string;
  platform: ImPlatform;
  appId: string;
  appSecret: string;
  label?: string | null;
  enabled: boolean;
  assignment?: Assignment | null;
  sandbox?: boolean | null;
};

type PlatformStatus = {
  running: boolean;
  parent_port?: number | null;
  config_path: string;
  log_path: string;
};

type ChannelFormValue = {
  platform: ImPlatform;
  label?: string;
  appId: string;
  appSecret: string;
  enabled: boolean;
  assignment: Assignment;
  sandbox?: boolean;
};

const PLATFORM_META: Record<ImPlatform, { label: string; color: string; appIdPlaceholder: string; secretPlaceholder: string; docsUrl: string }> = {
  feishu: {
    label: '飞书',
    color: 'blue',
    appIdPlaceholder: 'cli_xxxxxxxxxxxxxxxx',
    secretPlaceholder: '飞书应用 App Secret',
    docsUrl: 'https://open.feishu.cn/app',
  },
  qq: {
    label: 'QQ',
    color: 'cyan',
    appIdPlaceholder: '1020xxxxxx',
    secretPlaceholder: 'QQ 机器人 App Secret',
    docsUrl: 'https://q.qq.com',
  },
};

function createChannel(platform: ImPlatform = 'feishu'): ImChannel {
  return {
    id: `${platform}-${crypto.randomUUID()}`,
    platform,
    appId: '',
    appSecret: '',
    label: '',
    enabled: true,
    assignment: 'aiagent',
    sandbox: false,
  };
}

function normalizeAssignment(value?: string | null): Assignment {
  if (value === 'native_cli' || value === 'none') return 'native_cli';
  return 'aiagent';
}

function platformTag(platform: ImPlatform) {
  const meta = PLATFORM_META[platform];
  return <Tag color={meta.color}>{meta.label}</Tag>;
}

function maskSecret(secret: string) {
  if (!secret) return '未填写 App Secret';
  if (secret.length <= 8) return '********';
  return `${secret.slice(0, 4)}****${secret.slice(-4)}`;
}

export function IMChannelsPage() {
  const { message, modal } = App.useApp();
  const { token } = theme.useToken();
  const [channels, setChannels] = useState<ImChannel[]>([]);
  const [status, setStatus] = useState<PlatformStatus | null>(null);
  const [loading, setLoading] = useState(false);
  const [saving, setSaving] = useState(false);
  const [editing, setEditing] = useState<ImChannel | null>(null);
  const [form] = Form.useForm<ChannelFormValue>();
  const selectedPlatform = Form.useWatch('platform', form) ?? 'feishu';

  const activeCount = useMemo(
    () => channels.filter((ch) => ch.enabled).length,
    [channels],
  );

  const load = async () => {
    setLoading(true);
    try {
      const [remoteChannels, remoteStatus] = await Promise.all([
        invoke<ImChannel[]>('get_im_channels'),
        invoke<PlatformStatus>('platform_status'),
      ]);
      setChannels(remoteChannels.map((ch) => ({
        ...ch,
        platform: ch.platform === 'qq' ? 'qq' : 'feishu',
        enabled: ch.enabled !== false,
        assignment: normalizeAssignment(ch.assignment),
        sandbox: Boolean(ch.sandbox),
      })));
      setStatus(remoteStatus);
    } catch (err) {
      message.error(String(err));
    } finally {
      setLoading(false);
    }
  };

  useEffect(() => {
    void load();
  }, []);

  const applySidecarConfig = async () => {
    try {
      const remoteStatus = await invoke<PlatformStatus>('platform_status');
      if (remoteStatus.running) {
        await invoke('platform_reload_config');
      } else {
        await invoke('platform_start');
      }
      setStatus(await invoke<PlatformStatus>('platform_status'));
    } catch (err) {
      message.warning(`通道已保存，但 sidecar 刷新失败：${String(err)}`);
    }
  };

  const saveChannels = async (next: ImChannel[]) => {
    await invoke('save_im_channels', { channels: next });
    setChannels(next);
    await applySidecarConfig();
  };

  const openEditor = (record?: ImChannel, platform: ImPlatform = 'feishu') => {
    const value = record ?? createChannel(platform);
    setEditing(value);
    form.setFieldsValue({
      platform: value.platform,
      label: value.label ?? '',
      appId: value.appId,
      appSecret: value.appSecret,
      enabled: value.enabled !== false,
      assignment: normalizeAssignment(value.assignment),
      sandbox: Boolean(value.sandbox),
    });
  };

  const submitEditor = async () => {
    const values = await form.validateFields();
    if (!editing) return;

    setSaving(true);
    try {
      const platform = values.platform;
      const appId = values.appId.trim();
      const nextChannel: ImChannel = {
        ...editing,
        id: editing.appId && editing.platform === platform ? editing.id : `${platform}-${appId}`,
        platform,
        label: values.label?.trim() || '',
        appId,
        appSecret: values.appSecret.trim(),
        enabled: values.enabled,
        assignment: values.assignment,
        sandbox: platform === 'qq' ? Boolean(values.sandbox) : false,
      };

      const next = channels.some((ch) => ch.id === editing.id)
        ? channels.map((ch) => (ch.id === editing.id ? nextChannel : ch))
        : [...channels, nextChannel];

      await saveChannels(next);
      setEditing(null);
      message.success('IM 通道已保存');
    } finally {
      setSaving(false);
    }
  };

  const confirmRemove = (record: ImChannel) => {
    modal.confirm({
      title: `删除 ${PLATFORM_META[record.platform].label} 通道？`,
      content: record.appId,
      okText: '删除',
      okButtonProps: { danger: true },
      cancelText: '取消',
      onOk: async () => {
        await saveChannels(channels.filter((ch) => ch.id !== record.id));
        message.success('IM 通道已删除');
      },
    });
  };

  const patchChannel = async (record: ImChannel, patch: Partial<ImChannel>) => {
    await saveChannels(channels.map((ch) => (ch.id === record.id ? { ...ch, ...patch } : ch)));
  };

  const columns: ColumnsType<ImChannel> = [
    {
      title: '通道',
      dataIndex: 'label',
      render: (_value, record) => (
        <Space direction="vertical" size={0}>
          <Space size={8}>
            <Typography.Text strong>{record.label || `${PLATFORM_META[record.platform].label} 机器人`}</Typography.Text>
            {platformTag(record.platform)}
            {record.platform === 'qq' && record.sandbox ? <Tag color="orange">沙箱</Tag> : null}
          </Space>
          <Typography.Text type="secondary" style={{ fontSize: 12 }}>
            {record.appId || '未填写 App ID'} · {maskSecret(record.appSecret)}
          </Typography.Text>
        </Space>
      ),
    },
    {
      title: '状态',
      width: 130,
      render: (_, record) => (
        <Switch
          checked={record.enabled}
          checkedChildren="启用"
          unCheckedChildren="停用"
          onChange={(checked) => void patchChannel(record, { enabled: checked })}
        />
      ),
    },
    {
      title: '后端',
      width: 170,
      render: (_, record) => (
        <Select<Assignment>
          size="small"
          value={normalizeAssignment(record.assignment)}
          style={{ width: 130 }}
          options={[
            { value: 'aiagent', label: 'AI Agent' },
            { value: 'native_cli', label: '原生 CLI' },
          ]}
          onChange={(assignment) => void patchChannel(record, { assignment })}
        />
      ),
    },
    {
      title: '操作',
      width: 130,
      align: 'right',
      render: (_, record) => (
        <Space>
          <Tooltip title="编辑">
            <Button size="small" icon={<Edit3 size={14} />} onClick={() => openEditor(record)} />
          </Tooltip>
          <Tooltip title="删除">
            <Button size="small" danger icon={<Trash2 size={14} />} onClick={() => confirmRemove(record)} />
          </Tooltip>
        </Space>
      ),
    },
  ];

  return (
    <div style={{ height: '100%', overflow: 'auto', background: token.colorBgLayout }}>
      <div style={{ maxWidth: 1120, margin: '0 auto', padding: 24 }}>
        <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center', marginBottom: 16 }}>
          <Space align="center">
            <span style={{ width: 34, height: 34, display: 'inline-flex', alignItems: 'center', justifyContent: 'center', borderRadius: token.borderRadius, background: token.colorPrimaryBg, color: token.colorPrimary }}>
              <MessageCircle size={18} />
            </span>
            <div>
              <Typography.Title level={4} style={{ margin: 0 }}>IM 通道</Typography.Title>
              <Typography.Text type="secondary">配置飞书和 QQ 机器人，消息可进入 AI Agent 或原生 CLI。</Typography.Text>
            </div>
          </Space>
          <Space>
            {status?.running ? <Tag color="green">Sidecar 已自动运行</Tag> : <Tag color="red">Sidecar 未运行</Tag>}
            <Typography.Text type="secondary">已启用 {activeCount} 个通道</Typography.Text>
            <Button icon={<RefreshCw size={16} />} onClick={() => void load()} loading={loading}>刷新</Button>
            <Button icon={<Bot size={16} />} onClick={() => openEditor(undefined, 'qq')}>添加 QQ</Button>
            <Button type="primary" icon={<Plus size={16} />} onClick={() => openEditor(undefined, 'feishu')}>添加飞书</Button>
          </Space>
        </div>

        <Card styles={{ body: { padding: 0 } }}>
          <Table
            rowKey="id"
            columns={columns}
            dataSource={channels}
            loading={loading}
            pagination={false}
            locale={{ emptyText: <Empty description="还没有 IM 通道" /> }}
          />
        </Card>
      </div>

      <Modal
        title={editing?.appId ? '编辑 IM 通道' : `添加 ${PLATFORM_META[selectedPlatform].label} 通道`}
        open={!!editing}
        onCancel={() => setEditing(null)}
        onOk={() => void submitEditor()}
        okText="保存"
        confirmLoading={saving}
        okButtonProps={{ icon: <Save size={15} /> }}
        destroyOnHidden
      >
        <Form form={form} layout="vertical" initialValues={{ platform: 'feishu', enabled: true, assignment: 'aiagent', sandbox: false }}>
          <Form.Item label="平台" name="platform" rules={[{ required: true }]}>
            <Select
              options={[
                { value: 'feishu', label: '飞书' },
                { value: 'qq', label: 'QQ' },
              ]}
            />
          </Form.Item>
          <Form.Item label="名称" name="label">
            <Input placeholder={`${PLATFORM_META[selectedPlatform].label} 机器人`} />
          </Form.Item>
          <Form.Item label="App ID" name="appId" rules={[{ required: true, message: '请输入 App ID' }]}>
            <Input placeholder={PLATFORM_META[selectedPlatform].appIdPlaceholder} />
          </Form.Item>
          <Form.Item label="App Secret" name="appSecret" rules={[{ required: true, message: '请输入 App Secret' }]}>
            <Input.Password placeholder={PLATFORM_META[selectedPlatform].secretPlaceholder} />
          </Form.Item>
          {selectedPlatform === 'qq' ? (
            <Form.Item label="沙箱模式" name="sandbox" valuePropName="checked">
              <Switch checkedChildren="开启" unCheckedChildren="关闭" />
            </Form.Item>
          ) : null}
          <Form.Item label="后端" name="assignment" rules={[{ required: true }]}>
            <Select
              options={[
                { value: 'aiagent', label: 'AI Agent' },
                { value: 'native_cli', label: '原生 CLI' },
              ]}
            />
          </Form.Item>
          <Form.Item label="启用" name="enabled" valuePropName="checked">
            <Switch checkedChildren="启用" unCheckedChildren="停用" />
          </Form.Item>

          <Space direction="vertical" size={4}>
            <Button
              type="link"
              size="small"
              icon={<ExternalLink size={13} />}
              style={{ padding: 0 }}
              onClick={() => void openUrl(PLATFORM_META[selectedPlatform].docsUrl)}
            >
              打开 {PLATFORM_META[selectedPlatform].label} 开放平台
            </Button>
            <Typography.Text type="secondary" style={{ fontSize: 12 }}>
              <CircleOff size={12} style={{ verticalAlign: -2, marginRight: 4 }} />
              AI Agent 使用 Codex app server，原生 CLI 使用本机 CLI 引擎。
            </Typography.Text>
          </Space>
        </Form>
      </Modal>
    </div>
  );
}
