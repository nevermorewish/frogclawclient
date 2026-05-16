import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react';
import { App, Button, Empty, Form, Input, Modal, Spin, Table, Tag, Tooltip, Typography, theme } from 'antd';
import type { ColumnsType } from 'antd/es/table';
import {
  Activity,
  Brain,
  Clipboard,
  Database,
  FolderOpen,
  Plus,
  RefreshCw,
  Search,
  Sparkles,
} from 'lucide-react';
import { invoke } from '@/lib/invoke';
import { useMemoryStore } from '@/stores';
import type { MemoryItem, ProjectMemoryProfile } from '@/types';

interface VectorSearchResult {
  id: string;
  document_id: string;
  chunk_index: number;
  content: string;
  score: number;
  rerankScore?: number;
  has_embedding?: boolean;
}

function formatDate(value?: string): string {
  if (!value) return '-';
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return value;
  return date.toLocaleString();
}

function formatScore(result: VectorSearchResult): string {
  if (typeof result.rerankScore === 'number') {
    return result.rerankScore.toFixed(4);
  }
  return (1 / (1 + Math.max(0, result.score))).toFixed(4);
}

function sourceLabel(source: string): string {
  if (source === 'auto_extract') return '自动捕获';
  if (source === 'manual') return '手动写入';
  return source || 'Claude-Mem';
}

function sourceColor(source: string): string {
  if (source === 'auto_extract') return 'green';
  if (source === 'manual') return 'blue';
  return 'default';
}

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
        <span
          className="min-w-0 flex-1 truncate"
          style={{ fontWeight: 600, color: active ? token.colorPrimary : token.colorText }}
        >
          {profile.projectName}
        </span>
      </div>
      <div className="mt-1 flex items-center gap-2" style={{ color: token.colorTextSecondary, fontSize: 12 }}>
        <Tag color="processing" style={{ margin: 0, fontSize: 11 }}>Claude-Mem</Tag>
        <span>{profile.itemCount > 0 ? `${profile.itemCount} 条记忆` : '按项目召回'}</span>
      </div>
      <div className="mt-1 truncate" style={{ color: token.colorTextTertiary, fontSize: 11 }}>
        {profile.projectPath}
      </div>
    </button>
  );
}

export default function ProjectMemorySettings() {
  const { token } = theme.useToken();
  const { message } = App.useApp();
  const {
    projectProfiles,
    selectedProjectPath,
    items,
    loading,
    error,
    loadProjectProfiles,
    setSelectedProjectPath,
    loadProjectItems,
    addProjectItem,
  } = useMemoryStore();

  const [itemModalOpen, setItemModalOpen] = useState(false);
  const [savingItem, setSavingItem] = useState(false);
  const [summarizing, setSummarizing] = useState(false);
  const [searchQuery, setSearchQuery] = useState('');
  const [searching, setSearching] = useState(false);
  const [searchResults, setSearchResults] = useState<VectorSearchResult[] | null>(null);
  const tableWrapRef = useRef<HTMLDivElement | null>(null);
  const [tableScrollY, setTableScrollY] = useState<number>(360);
  const [itemForm] = Form.useForm<{ title: string; content: string }>();

  useEffect(() => {
    void loadProjectProfiles();
  }, [loadProjectProfiles]);

  const selectedProfile = useMemo(
    () => projectProfiles.find((profile) => profile.projectPath === selectedProjectPath) ?? projectProfiles[0] ?? null,
    [projectProfiles, selectedProjectPath],
  );
  const selectedProfilePath = selectedProfile?.projectPath ?? null;
  const selectedProfileName = selectedProfile?.projectName ?? null;

  useEffect(() => {
    if (!selectedProfilePath || !selectedProfileName) return;
    if (selectedProjectPath !== selectedProfilePath) {
      setSelectedProjectPath(selectedProfilePath);
    }
    void loadProjectItems(selectedProfilePath, selectedProfileName);
  }, [loadProjectItems, selectedProfileName, selectedProfilePath, selectedProjectPath, setSelectedProjectPath]);

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
  }, [selectedProfilePath]);

  const refreshSelectedProject = useCallback(async () => {
    await loadProjectProfiles();
    if (selectedProfilePath && selectedProfileName) {
      await loadProjectItems(selectedProfilePath, selectedProfileName);
    }
  }, [loadProjectItems, loadProjectProfiles, selectedProfileName, selectedProfilePath]);

  const openAddItem = () => {
    itemForm.resetFields();
    setItemModalOpen(true);
  };

  const handleAddItem = async () => {
    if (!selectedProfile) return;
    const values = await itemForm.validateFields();
    setSavingItem(true);
    try {
      await addProjectItem(selectedProfile.projectPath, selectedProfile.projectName, values.title, values.content);
      itemForm.resetFields();
      setItemModalOpen(false);
      message.success('已写入 claude-mem');
    } catch (e) {
      message.error(`写入失败：${String(e)}`);
    } finally {
      setSavingItem(false);
    }
  };

  const handleSummarizeConversation = async () => {
    if (!selectedProfile) return;
    setSummarizing(true);
    try {
      const count = await invoke<number>('summarize_project_memory', {
        projectPath: selectedProfile.projectPath,
        projectName: selectedProfile.projectName,
      });
      await loadProjectProfiles();
      await loadProjectItems(selectedProfile.projectPath, selectedProfile.projectName);
      message.success(count > 0 ? `已写入 ${count} 条会话摘要` : '本次没有新的会话摘要可写入');
    } catch (e) {
      message.error(`总结失败：${String(e)}`);
    } finally {
      setSummarizing(false);
    }
  };

  const handleSearch = useCallback(async (value?: string) => {
    if (!selectedProfile) return;
    const query = (value ?? searchQuery).trim();
    if (!query) {
      setSearchResults(null);
      return;
    }
    setSearching(true);
    try {
      const results = await invoke<VectorSearchResult[]>('search_project_memory', {
        projectPath: selectedProfile.projectPath,
        projectName: selectedProfile.projectName,
        query,
        topK: 8,
      });
      setSearchResults(results);
    } catch (e) {
      message.error(`搜索失败：${String(e)}`);
    } finally {
      setSearching(false);
    }
  }, [message, searchQuery, selectedProfile]);

  const copyMemory = async (content: string) => {
    await navigator.clipboard.writeText(content);
    message.success('已复制');
  };

  const columns: ColumnsType<MemoryItem> = [
    {
      title: '记忆',
      dataIndex: 'content',
      render: (_value: string, record) => (
        <div className="min-w-0">
          <Typography.Text strong>{record.title}</Typography.Text>
          <Typography.Paragraph
            ellipsis={{ rows: 2, tooltip: record.content }}
            style={{ margin: '4px 0 0', color: token.colorTextSecondary }}
          >
            {record.content}
          </Typography.Paragraph>
        </div>
      ),
    },
    {
      title: '来源',
      dataIndex: 'source',
      width: 112,
      render: (value: string) => <Tag color={sourceColor(value)}>{sourceLabel(value)}</Tag>,
    },
    {
      title: '写入时间',
      dataIndex: 'updatedAt',
      width: 188,
      render: (value: string) => formatDate(value),
    },
    {
      title: '操作',
      key: 'actions',
      width: 80,
      fixed: 'right',
      render: (_value, record) => (
        <Tooltip title="复制内容">
          <Button
            type="text"
            size="small"
            icon={<Clipboard size={14} />}
            onClick={() => void copyMemory(record.content)}
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
        <div className="p-3" style={{ borderBottom: `1px solid ${token.colorBorderSecondary}` }}>
          <div className="flex items-center justify-between">
            <div className="flex items-center gap-2">
              <Brain size={17} />
              <strong>项目记忆</strong>
            </div>
            <Tooltip title="刷新项目">
              <Button size="small" icon={<RefreshCw size={14} />} onClick={() => void refreshSelectedProject()} />
            </Tooltip>
          </div>
          <div className="mt-2 flex items-center gap-2" style={{ color: token.colorTextSecondary, fontSize: 12 }}>
            <Activity size={13} />
            <span>本机 claude-mem worker · 127.0.0.1:37777</span>
          </div>
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
                <div className="flex flex-wrap items-center gap-2">
                  <h2 style={{ margin: 0, fontSize: 20 }}>{selectedProfile.projectName}</h2>
                  <Tag color="processing">Claude-Mem</Tag>
                  <Tag color="success">自动召回</Tag>
                </div>
                <div className="mt-1 truncate" style={{ color: token.colorTextSecondary, fontSize: 12 }}>
                  {selectedProfile.projectPath}
                </div>
              </div>
              <div className="flex flex-wrap gap-2">
                <Button icon={<RefreshCw size={15} />} onClick={() => void refreshSelectedProject()}>
                  刷新
                </Button>
                <Button
                  icon={<Sparkles size={15} />}
                  loading={summarizing}
                  onClick={() => void handleSummarizeConversation()}
                >
                  从会话提取
                </Button>
                <Button type="primary" icon={<Plus size={15} />} onClick={openAddItem}>
                  添加记忆
                </Button>
              </div>
            </div>

            {error ? (
              <div
                className="mb-4 rounded-md border p-3"
                style={{ borderColor: token.colorErrorBorder, background: token.colorErrorBg }}
              >
                <Typography.Text type="danger">{error}</Typography.Text>
              </div>
            ) : null}

            <div className="mb-4 grid shrink-0 grid-cols-1 gap-3 lg:grid-cols-[minmax(0,1fr)_340px]">
              <div className="rounded-md border p-3" style={{ borderColor: token.colorBorderSecondary }}>
                <div className="mb-2 flex items-center gap-2">
                  <Search size={15} />
                  <strong>语义搜索</strong>
                </div>
                <Input.Search
                  allowClear
                  value={searchQuery}
                  placeholder="搜索当前项目写入 claude-mem 的记忆"
                  enterButton="搜索"
                  loading={searching}
                  onChange={(event) => {
                    setSearchQuery(event.target.value);
                    if (!event.target.value.trim()) setSearchResults(null);
                  }}
                  onSearch={(value) => void handleSearch(value)}
                />
                <div className="mt-3 max-h-[190px] overflow-y-auto">
                  {searchResults === null ? (
                    <Typography.Text type="secondary">搜索会调用 claude-mem 的观察检索，并按当前项目过滤。</Typography.Text>
                  ) : searchResults.length === 0 ? (
                    <Empty image={Empty.PRESENTED_IMAGE_SIMPLE} description="没有匹配的记忆" />
                  ) : (
                    <div className="flex flex-col gap-2">
                      {searchResults.map((result) => (
                        <div
                          key={result.id}
                          className="rounded-md border p-2"
                          style={{ borderColor: token.colorBorderSecondary }}
                        >
                          <div className="mb-1 flex items-center justify-between gap-2">
                            <Typography.Text code>{result.document_id.slice(0, 12)}</Typography.Text>
                            <Tag color="blue" style={{ margin: 0 }}>{formatScore(result)}</Tag>
                          </div>
                          <Typography.Paragraph ellipsis={{ rows: 2, tooltip: result.content }} style={{ margin: 0 }}>
                            {result.content}
                          </Typography.Paragraph>
                        </div>
                      ))}
                    </div>
                  )}
                </div>
              </div>

              <div className="rounded-md border p-3" style={{ borderColor: token.colorBorderSecondary }}>
                <div style={{ color: token.colorTextSecondary, fontSize: 12 }}>当前项目观察</div>
                <div className="mt-2 flex items-center gap-2">
                  <Database size={18} />
                  <strong style={{ fontSize: 24 }}>{items.length}</strong>
                  <span style={{ color: token.colorTextSecondary }}>条</span>
                </div>
                <div className="mt-3" style={{ color: token.colorTextSecondary, fontSize: 12, lineHeight: 1.7 }}>
                  记忆由本机 claude-mem 保存和索引。FrogClaw 只负责写入、搜索和在聊天时按项目注入上下文。
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
                locale={{ emptyText: '暂无 claude-mem 记忆' }}
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
        okText="写入"
        confirmLoading={savingItem}
        mask={{ blur: true }}
      >
        <Form form={itemForm} layout="vertical">
          <Form.Item name="title" label="标题" rules={[{ required: true, message: '请输入标题' }]}>
            <Input placeholder="例如：项目打包规则" />
          </Form.Item>
          <Form.Item name="content" label="内容" rules={[{ required: true, message: '请输入内容' }]}>
            <Input.TextArea rows={6} placeholder="写入这个项目后续需要自动召回的事实、约束、路径或决策" />
          </Form.Item>
        </Form>
      </Modal>
    </div>
  );
}
