import { useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react';
import { App, Button, Empty, Form, Input, InputNumber, Modal, Spin, Table, Tag, Tooltip, theme } from 'antd';
import type { ColumnsType } from 'antd/es/table';
import { Brain, CheckCircle2, Clock3, Database, FolderOpen, Plus, RefreshCw, Settings, Sparkles, Trash2, TriangleAlert } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { useMemoryStore, useSettingsStore } from '@/stores';
import { EmbeddingModelSelect } from '@/components/shared/EmbeddingModelSelect';
import { invoke } from '@/lib/invoke';
import type { MemoryItem, ProjectMemoryProfile } from '@/types';

const INDEX_STATUS_COLOR: Record<string, string> = {
  pending: 'default',
  indexing: 'processing',
  ready: 'success',
  failed: 'error',
  skipped: 'warning',
};

function ProjectRow({
  profile,
  active,
  onClick,
}: {
  profile: ProjectMemoryProfile;
  active: boolean;
  onClick: () => void;
}) {
  const { token } = theme.useToken();
  return (
    <button
      type="button"
      onClick={onClick}
      className="w-full text-left"
      style={{
        border: `1px solid ${active ? token.colorPrimaryBorder : 'transparent'}`,
        background: active ? token.colorPrimaryBg : 'transparent',
        borderRadius: token.borderRadius,
        padding: 10,
        cursor: 'pointer',
      }}
    >
      <div className="flex items-center gap-2">
        <FolderOpen size={16} color={active ? token.colorPrimary : token.colorTextSecondary} />
        <span className="min-w-0 flex-1 truncate" style={{ fontWeight: 600, color: active ? token.colorPrimary : token.colorText }}>
          {profile.projectName}
        </span>
      </div>
      <div className="mt-1 truncate" style={{ color: token.colorTextSecondary, fontSize: 12 }}>
        {profile.itemCount} 条记忆 · {profile.pendingCount} 待处理 · {profile.failedCount} 失败
      </div>
      <div className="mt-1 truncate" style={{ color: token.colorTextTertiary, fontSize: 11 }}>
        {profile.projectPath}
      </div>
    </button>
  );
}

export default function ProjectMemorySettings() {
  const { t } = useTranslation();
  const { token } = theme.useToken();
  const { message, modal } = App.useApp();
  const settings = useSettingsStore((s) => s.settings);
  const {
    projectProfiles,
    selectedProjectPath,
    items,
    loading,
    loadProjectProfiles,
    setSelectedProjectPath,
    loadProjectItems,
    addProjectItem,
    updateProjectProfile,
  } = useMemoryStore();

  const [itemModalOpen, setItemModalOpen] = useState(false);
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [summarizing, setSummarizing] = useState(false);
  const tableWrapRef = useRef<HTMLDivElement | null>(null);
  const [tableScrollY, setTableScrollY] = useState<number>(360);
  const [itemForm] = Form.useForm<{ title: string; content: string }>();
  const [settingsForm] = Form.useForm<{
    embeddingProvider?: string;
    embeddingDimensions?: number;
    retrievalTopK?: number;
    retrievalThreshold?: number;
  }>();

  useEffect(() => {
    void loadProjectProfiles();
  }, [loadProjectProfiles]);

  const selectedProfile = useMemo(
    () => projectProfiles.find((profile) => profile.projectPath === selectedProjectPath) ?? projectProfiles[0] ?? null,
    [projectProfiles, selectedProjectPath],
  );

  useEffect(() => {
    if (!selectedProfile) return;
    if (selectedProjectPath !== selectedProfile.projectPath) {
      setSelectedProjectPath(selectedProfile.projectPath);
    }
    void loadProjectItems(selectedProfile.projectPath, selectedProfile.projectName);
  }, [loadProjectItems, selectedProfile, selectedProjectPath, setSelectedProjectPath]);

  useLayoutEffect(() => {
    const element = tableWrapRef.current;
    if (!element) return;
    const updateHeight = () => {
      const rect = element.getBoundingClientRect();
      setTableScrollY(Math.max(220, Math.floor(rect.height - 64)));
    };
    updateHeight();
    const observer = new ResizeObserver(updateHeight);
    observer.observe(element);
    window.addEventListener('resize', updateHeight);
    return () => {
      observer.disconnect();
      window.removeEventListener('resize', updateHeight);
    };
  }, [selectedProfile]);

  const openSettings = () => {
    if (!selectedProfile) return;
    const defaultEmbeddingProvider = settings.default_embedding_provider_id && settings.default_embedding_model_id
      ? `${settings.default_embedding_provider_id}::${settings.default_embedding_model_id}`
      : undefined;
    settingsForm.setFieldsValue({
      embeddingProvider: selectedProfile.embeddingProvider ?? defaultEmbeddingProvider,
      embeddingDimensions: selectedProfile.embeddingDimensions,
      retrievalTopK: selectedProfile.retrievalTopK ?? 6,
      retrievalThreshold: selectedProfile.retrievalThreshold ?? 0.35,
    });
    setSettingsOpen(true);
  };

  const handleSaveSettings = async () => {
    if (!selectedProfile) return;
    const values = await settingsForm.validateFields();
    await updateProjectProfile(selectedProfile.projectPath, selectedProfile.projectName, {
      embeddingProvider: values.embeddingProvider,
      updateEmbeddingProvider: true,
      embeddingDimensions: values.embeddingDimensions,
      updateEmbeddingDimensions: true,
      retrievalTopK: values.retrievalTopK,
      updateRetrievalTopK: true,
      retrievalThreshold: values.retrievalThreshold,
      updateRetrievalThreshold: true,
    });
    setSettingsOpen(false);
    message.success('项目记忆设置已保存');
  };

  const handleAddItem = async () => {
    if (!selectedProfile) return;
    const values = await itemForm.validateFields();
    await addProjectItem(selectedProfile.projectPath, selectedProfile.projectName, values.title, values.content);
    itemForm.resetFields();
    setItemModalOpen(false);
    message.success('记忆已添加');
  };

  const handleSummarizeConversation = async () => {
    if (!selectedProfile) return;
    setSummarizing(true);
    try {
      const count = await invoke<number>('summarize_project_memory', {
        projectPath: selectedProfile.projectPath,
        projectName: selectedProfile.projectName,
      });
      await loadProjectItems(selectedProfile.projectPath, selectedProfile.projectName);
      await loadProjectProfiles();
      message.success(count > 0 ? `已总结 ${count} 条会话记忆` : '本次会话没有可新增的项目记忆');
    } catch (e) {
      message.error(`总结失败：${String(e)}`);
    } finally {
      setSummarizing(false);
    }
  };

  const handleDeleteItem = (item: MemoryItem) => {
    if (!selectedProfile) return;
    modal.confirm({
      title: '删除这条项目记忆？',
      content: item.title,
      okButtonProps: { danger: true },
      mask: { blur: true },
      onOk: async () => {
        await invoke('delete_memory_item', {
          namespaceId: selectedProfile.namespaceId,
          id: item.id,
        });
        await loadProjectItems(selectedProfile.projectPath, selectedProfile.projectName);
        await loadProjectProfiles();
        message.success('记忆已删除');
      },
    });
  };

  const columns: ColumnsType<MemoryItem> = [
    {
      title: '标题',
      dataIndex: 'title',
      width: 260,
      render: (value: string) => <strong>{value}</strong>,
    },
    {
      title: '内容',
      dataIndex: 'content',
      ellipsis: true,
      render: (value: string) => (
        <Tooltip title={value}>
          <span>{value}</span>
        </Tooltip>
      ),
    },
    {
      title: '来源',
      dataIndex: 'source',
      width: 110,
      render: (value: string) => <Tag color={value === 'auto_extract' ? 'green' : 'blue'}>{value}</Tag>,
    },
    {
      title: '索引',
      dataIndex: 'indexStatus',
      width: 110,
      render: (value: string, record) => (
        <Tooltip title={record.indexError}>
          <Tag color={INDEX_STATUS_COLOR[value] ?? 'default'}>{value}</Tag>
        </Tooltip>
      ),
    },
    {
      title: '更新时间',
      dataIndex: 'updatedAt',
      width: 190,
    },
    {
      title: '操作',
      key: 'actions',
      width: 82,
      fixed: 'right',
      render: (_value, record) => (
        <Tooltip title="删除">
          <Button
            type="text"
            danger
            size="small"
            icon={<Trash2 size={14} />}
            onClick={() => handleDeleteItem(record)}
          />
        </Tooltip>
      ),
    },
  ];

  return (
    <div className="flex h-full min-h-0" style={{ background: token.colorBgContainer }}>
      <aside
        className="flex h-full min-h-0 w-[310px] shrink-0 flex-col"
        style={{ borderRight: `1px solid ${token.colorBorderSecondary}` }}
      >
        <div className="flex items-center justify-between p-3" style={{ borderBottom: `1px solid ${token.colorBorderSecondary}` }}>
          <div className="flex items-center gap-2">
            <Brain size={17} />
            <strong>项目记忆</strong>
          </div>
          <Button size="small" icon={<RefreshCw size={14} />} onClick={() => void loadProjectProfiles()} />
        </div>
        <div className="min-h-0 flex-1 overflow-y-auto p-2">
          {projectProfiles.length === 0 && !loading ? (
            <Empty image={Empty.PRESENTED_IMAGE_SIMPLE} description="暂无项目" />
          ) : (
            <div className="flex flex-col gap-2">
              {projectProfiles.map((profile) => (
                <ProjectRow
                  key={profile.projectPath}
                  profile={profile}
                  active={selectedProfile?.projectPath === profile.projectPath}
                  onClick={() => setSelectedProjectPath(profile.projectPath)}
                />
              ))}
            </div>
          )}
        </div>
      </aside>

      <main className="min-w-0 flex-1 overflow-hidden p-4">
        {!selectedProfile ? (
          <div className="flex h-full items-center justify-center">
            <Spin spinning={loading}>
              <Empty description="请选择项目" />
            </Spin>
          </div>
        ) : (
          <div className="mx-auto flex h-full min-h-0 max-w-[1180px] flex-col">
            <div className="mb-4 flex shrink-0 flex-wrap items-start justify-between gap-4">
              <div className="min-w-0">
                <h2 style={{ margin: 0, fontSize: 20 }}>{selectedProfile.projectName}</h2>
                <div className="truncate" style={{ color: token.colorTextSecondary, fontSize: 12 }}>{selectedProfile.projectPath}</div>
              </div>
              <div className="flex flex-wrap gap-2">
                <Button icon={<Settings size={15} />} onClick={openSettings}>项目设置</Button>
                <Button
                  icon={<Sparkles size={15} />}
                  loading={summarizing}
                  onClick={() => void handleSummarizeConversation()}
                >
                  手动总结会话记忆
                </Button>
                <Button type="primary" icon={<Plus size={15} />} onClick={() => setItemModalOpen(true)}>添加记忆</Button>
              </div>
            </div>

            <div className="mb-4 grid shrink-0 grid-cols-1 gap-3 md:grid-cols-2 xl:grid-cols-4">
              <div className="rounded-md border p-3" style={{ borderColor: token.colorBorderSecondary }}>
                <div style={{ color: token.colorTextSecondary, fontSize: 12 }}>有效记忆</div>
                <div className="mt-1 flex items-center gap-2"><Database size={16} /><strong style={{ fontSize: 20 }}>{selectedProfile.itemCount}</strong></div>
              </div>
              <div className="rounded-md border p-3" style={{ borderColor: token.colorBorderSecondary }}>
                <div style={{ color: token.colorTextSecondary, fontSize: 12 }}>待处理</div>
                <div className="mt-1 flex items-center gap-2"><Clock3 size={16} /><strong style={{ fontSize: 20 }}>{selectedProfile.pendingCount}</strong></div>
              </div>
              <div className="rounded-md border p-3" style={{ borderColor: token.colorBorderSecondary }}>
                <div style={{ color: token.colorTextSecondary, fontSize: 12 }}>失败</div>
                <div className="mt-1 flex items-center gap-2"><TriangleAlert size={16} /><strong style={{ fontSize: 20 }}>{selectedProfile.failedCount}</strong></div>
              </div>
              <div className="rounded-md border p-3" style={{ borderColor: token.colorBorderSecondary }}>
                <div style={{ color: token.colorTextSecondary, fontSize: 12 }}>向量状态</div>
                <div className="mt-1 flex items-center gap-2">
                  <CheckCircle2 size={16} />
                  <Tag color={selectedProfile.embeddingProvider ? 'green' : 'default'} style={{ margin: 0 }}>
                    {selectedProfile.embeddingProvider ? '已配置' : '未配置'}
                  </Tag>
                </div>
              </div>
            </div>

            <div ref={tableWrapRef} className="min-h-0 flex-1">
              <Table
                rowKey="id"
                columns={columns}
                dataSource={items}
                loading={loading}
                pagination={{ pageSize: 12, showSizeChanger: false }}
                scroll={{ y: tableScrollY, x: 760 }}
                locale={{ emptyText: t('settings.memory.empty', '暂无记忆') }}
              />
            </div>
          </div>
        )}
      </main>

      <Modal
        title="添加项目记忆"
        open={itemModalOpen}
        onCancel={() => setItemModalOpen(false)}
        onOk={() => void handleAddItem()}
        okText="添加"
      >
        <Form form={itemForm} layout="vertical">
          <Form.Item name="title" label="标题" rules={[{ required: true, message: '请输入标题' }]}>
            <Input placeholder="例如：项目打包规则" />
          </Form.Item>
          <Form.Item name="content" label="内容" rules={[{ required: true, message: '请输入内容' }]}>
            <Input.TextArea rows={6} placeholder="记录这个项目后续需要自动召回的事实、约束、路径或决策" />
          </Form.Item>
        </Form>
      </Modal>

      <Modal
        title="项目记忆设置"
        open={settingsOpen}
        onCancel={() => setSettingsOpen(false)}
        onOk={() => void handleSaveSettings()}
        okText="保存"
      >
        <Form form={settingsForm} layout="vertical">
          <Form.Item name="embeddingProvider" label="向量模型">
            <EmbeddingModelSelect
              placeholder="选择向量模型"
              onChange={(value) => settingsForm.setFieldValue('embeddingProvider', value)}
            />
          </Form.Item>
          <Form.Item name="embeddingDimensions" label="嵌入维度">
            <InputNumber min={1} placeholder="自动" style={{ width: '100%' }} />
          </Form.Item>
          <Form.Item name="retrievalTopK" label="召回数量">
            <InputNumber min={1} max={30} style={{ width: '100%' }} />
          </Form.Item>
          <Form.Item name="retrievalThreshold" label="召回阈值">
            <InputNumber min={0} max={1} step={0.05} style={{ width: '100%' }} />
          </Form.Item>
        </Form>
      </Modal>
    </div>
  );
}
