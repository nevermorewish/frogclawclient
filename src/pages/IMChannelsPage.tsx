import { useEffect, useMemo, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { App, Button, Card, Empty, Form, Input, Modal, Space, Switch, Table, Tag, Typography, theme } from 'antd';
import type { ColumnsType } from 'antd/es/table';
import { MessageCircle, Play, Plus, RefreshCw, Save, Square, Trash2 } from 'lucide-react';

type ImChannel = {
  id: string;
  platform: string;
  appId: string;
  appSecret: string;
  label?: string | null;
  enabled: boolean;
  assignment?: string | null;
};

type PlatformStatus = {
  running: boolean;
  parent_port?: number | null;
  config_path: string;
  log_path: string;
};

type ChannelFormValue = {
  label?: string;
  appId: string;
  appSecret: string;
  enabled: boolean;
};

function newChannel(): ImChannel {
  return {
    id: `feishu-${crypto.randomUUID()}`,
    platform: 'feishu',
    appId: '',
    appSecret: '',
    label: '飞书',
    enabled: true,
    assignment: 'frogclaw',
  };
}

export function IMChannelsPage() {
  const { message } = App.useApp();
  const { token } = theme.useToken();
  const [channels, setChannels] = useState<ImChannel[]>([]);
  const [status, setStatus] = useState<PlatformStatus | null>(null);
  const [logs, setLogs] = useState('');
  const [loading, setLoading] = useState(false);
  const [editing, setEditing] = useState<ImChannel | null>(null);
  const [form] = Form.useForm<ChannelFormValue>();

  const enabledCount = useMemo(() => channels.filter((ch) => ch.enabled).length, [channels]);

  const load = async () => {
    setLoading(true);
    try {
      const [remoteChannels, remoteStatus, remoteLogs] = await Promise.all([
        invoke<ImChannel[]>('get_im_channels'),
        invoke<PlatformStatus>('platform_status'),
        invoke<string>('platform_read_log', { maxBytes: 48000 }),
      ]);
      setChannels(remoteChannels);
      setStatus(remoteStatus);
      setLogs(remoteLogs);
    } catch (err) {
      message.error(String(err));
    } finally {
      setLoading(false);
    }
  };

  useEffect(() => {
    void load();
  }, []);

  const saveChannels = async (next: ImChannel[]) => {
    await invoke('save_im_channels', { channels: next });
    setChannels(next);
  };

  const openEditor = (record?: ImChannel) => {
    const value = record ?? newChannel();
    setEditing(value);
    form.setFieldsValue({
      label: value.label ?? '',
      appId: value.appId,
      appSecret: value.appSecret,
      enabled: value.enabled,
    });
  };

  const submitEditor = async () => {
    const values = await form.validateFields();
    if (!editing) return;
    const nextChannel: ImChannel = {
      ...editing,
      platform: 'feishu',
      label: values.label?.trim() || '飞书',
      appId: values.appId.trim(),
      appSecret: values.appSecret.trim(),
      enabled: values.enabled,
      assignment: 'frogclaw',
    };
    const exists = channels.some((ch) => ch.id === nextChannel.id);
    const next = exists
      ? channels.map((ch) => (ch.id === nextChannel.id ? nextChannel : ch))
      : [...channels, nextChannel];
    await saveChannels(next);
    setEditing(null);
    message.success('IM 通道已保存');
  };

  const removeChannel = async (record: ImChannel) => {
    await saveChannels(channels.filter((ch) => ch.id !== record.id));
    message.success('IM 通道已删除');
  };

  const toggleEnabled = async (record: ImChannel, enabled: boolean) => {
    await saveChannels(channels.map((ch) => (ch.id === record.id ? { ...ch, enabled } : ch)));
  };

  const runCommand = async (command: 'platform_start' | 'platform_stop' | 'platform_reload_config') => {
    setLoading(true);
    try {
      const nextStatus = await invoke<PlatformStatus>(command);
      setStatus(nextStatus);
      setLogs(await invoke<string>('platform_read_log', { maxBytes: 48000 }));
      message.success(command === 'platform_stop' ? 'IM 通道已停止' : 'IM 通道已启动');
    } catch (err) {
      message.error(String(err));
    } finally {
      setLoading(false);
    }
  };

  const columns: ColumnsType<ImChannel> = [
    {
      title: '通道',
      dataIndex: 'label',
      render: (_value, record) => (
        <Space direction="vertical" size={0}>
          <Typography.Text strong>{record.label || '飞书'}</Typography.Text>
          <Typography.Text type="secondary" style={{ fontSize: 12 }}>
            {record.appId || '未填写 App ID'}
          </Typography.Text>
        </Space>
      ),
    },
    {
      title: '平台',
      width: 110,
      render: () => <Tag color="blue">飞书</Tag>,
    },
    {
      title: '连接',
      width: 120,
      render: (_, record) => (
        <Switch
          checked={record.enabled}
          checkedChildren="启用"
          unCheckedChildren="停用"
          onChange={(checked) => void toggleEnabled(record, checked)}
        />
      ),
    },
    {
      title: '目标',
      width: 150,
      render: () => <Tag color="green">项目对话</Tag>,
    },
    {
      title: '操作',
      width: 190,
      render: (_, record) => (
        <Space>
          <Button size="small" onClick={() => openEditor(record)}>
            编辑
          </Button>
          <Button size="small" danger icon={<Trash2 size={14} />} onClick={() => void removeChannel(record)} />
        </Space>
      ),
    },
  ];

  return (
    <div style={{ height: '100%', overflow: 'auto', background: token.colorBgLayout }}>
      <div style={{ maxWidth: 1180, margin: '0 auto', padding: 24 }}>
        <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center', marginBottom: 16 }}>
          <Space align="center">
            <span style={{ width: 34, height: 34, display: 'inline-flex', alignItems: 'center', justifyContent: 'center', borderRadius: token.borderRadius, background: token.colorPrimaryBg, color: token.colorPrimary }}>
              <MessageCircle size={18} />
            </span>
            <div>
              <Typography.Title level={4} style={{ margin: 0 }}>IM 通道</Typography.Title>
              <Typography.Text type="secondary">飞书消息会进入 FrogClawClient 项目对话，回复通过飞书流动卡片持续更新。</Typography.Text>
            </div>
          </Space>
          <Space>
            <Button icon={<RefreshCw size={16} />} onClick={() => void load()} loading={loading}>刷新</Button>
            <Button type="primary" icon={<Plus size={16} />} onClick={() => openEditor()}>添加飞书</Button>
          </Space>
        </div>

        <div style={{ display: 'grid', gridTemplateColumns: '1fr 320px', gap: 16, alignItems: 'start' }}>
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

          <Card title="运行状态" extra={status?.running ? <Tag color="green">运行中</Tag> : <Tag>已停止</Tag>}>
            <Space direction="vertical" style={{ width: '100%' }} size={12}>
              <Typography.Text type="secondary">启用通道：{enabledCount}</Typography.Text>
              <Typography.Text type="secondary" copyable={{ text: status?.config_path || '' }}>配置：{status?.config_path || '-'}</Typography.Text>
              <Typography.Text type="secondary" copyable={{ text: status?.log_path || '' }}>日志：{status?.log_path || '-'}</Typography.Text>
              <Space>
                <Button type="primary" icon={<Play size={15} />} onClick={() => void runCommand('platform_start')} loading={loading}>启动</Button>
                <Button icon={<Square size={15} />} onClick={() => void runCommand('platform_stop')} loading={loading}>停止</Button>
                <Button icon={<RefreshCw size={15} />} onClick={() => void runCommand('platform_reload_config')} loading={loading}>重载</Button>
              </Space>
              <Typography.Paragraph type="secondary" style={{ margin: 0, fontSize: 12 }}>
                飞书卡片支持 Markdown 风格内容和交互卡片块，不能直接嵌入项目内的完整富文本 React 渲染器；当前会把对话内容转换为飞书卡片 Markdown。
              </Typography.Paragraph>
            </Space>
          </Card>
        </div>

        <Card title="Sidecar 日志" style={{ marginTop: 16 }} extra={<Button size="small" icon={<RefreshCw size={14} />} onClick={() => void load()}>刷新日志</Button>}>
          <pre style={{ margin: 0, minHeight: 220, maxHeight: 360, overflow: 'auto', whiteSpace: 'pre-wrap', fontSize: 12, color: token.colorTextSecondary }}>
            {logs || '暂无日志'}
          </pre>
        </Card>
      </div>

      <Modal
        title={editing?.id && channels.some((ch) => ch.id === editing.id) ? '编辑飞书通道' : '添加飞书通道'}
        open={!!editing}
        onCancel={() => setEditing(null)}
        onOk={() => void submitEditor()}
        okText="保存"
        okButtonProps={{ icon: <Save size={15} /> }}
        destroyOnHidden
      >
        <Form form={form} layout="vertical" initialValues={{ enabled: true }}>
          <Form.Item label="名称" name="label">
            <Input placeholder="飞书机器人" />
          </Form.Item>
          <Form.Item label="App ID" name="appId" rules={[{ required: true, message: '请输入飞书 App ID' }]}>
            <Input placeholder="cli_xxx" />
          </Form.Item>
          <Form.Item label="App Secret" name="appSecret" rules={[{ required: true, message: '请输入飞书 App Secret' }]}>
            <Input.Password placeholder="飞书应用密钥" />
          </Form.Item>
          <Form.Item label="启用" name="enabled" valuePropName="checked">
            <Switch checkedChildren="启用" unCheckedChildren="停用" />
          </Form.Item>
        </Form>
      </Modal>
    </div>
  );
}
