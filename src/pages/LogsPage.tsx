import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { App as AntdApp, Button, Empty, Input, Segmented, Space, Tag, Typography, theme } from 'antd';
import { ArrowDown, Copy, FolderOpen, RefreshCw, ScrollText, Search } from 'lucide-react';
import { revealItemInDir } from '@tauri-apps/plugin-opener';
import { useCopyToClipboard } from '@/hooks/useCopyToClipboard';

type LogSource = {
  key: string;
  label: string;
  command: 'platform_read_log' | 'install_read_log';
  pathHint: string;
};

const SOURCES: LogSource[] = [
  {
    key: 'sidecar',
    label: 'Sidecar 日志',
    command: 'platform_read_log',
    pathHint: '~/.frogclaw/platform-sidecar.log',
  },
  {
    key: 'install',
    label: '安装日志',
    command: 'install_read_log',
    pathHint: '~/.frogclaw/install.log',
  },
];

const CATEGORY_FILTERS = [
  { key: 'sidecar', label: 'Sidecar', pattern: /sidecar|platform/i },
  { key: 'feishu', label: '飞书', pattern: /feishu|飞书/i },
  { key: 'qq', label: 'QQ', pattern: /qqbot|QQ|qq/i },
  { key: 'install', label: 'Install', pattern: /install|安装|node|git|claude|codex|gemini/i },
  { key: 'warn', label: 'Warn', pattern: /\bwarn\b|warning|警告/i },
  { key: 'error', label: 'Error', pattern: /\berror\b|failed|失败|\[err\]/i },
];

function lineColor(line: string, token: ReturnType<typeof theme.useToken>['token']) {
  if (/\berror\b|failed|失败|\[err\]/i.test(line)) return token.colorError;
  if (/\bwarn\b|warning|警告/i.test(line)) return token.colorWarning;
  if (/ready|connected|success|完成|成功/i.test(line)) return token.colorSuccess;
  if (/feishu|飞书/i.test(line)) return token.colorInfo;
  if (/qqbot|QQ|qq/i.test(line)) return token.colorPrimary;
  return '#9aa4b2';
}

export function LogsPage() {
  const { token } = theme.useToken();
  const { message } = AntdApp.useApp();
  const { copy } = useCopyToClipboard();
  const [activeKey, setActiveKey] = useState(SOURCES[0].key);
  const [rawLog, setRawLog] = useState('');
  const [loading, setLoading] = useState(false);
  const [filter, setFilter] = useState('');
  const [activeCategories, setActiveCategories] = useState<string[]>([]);
  const scrollRef = useRef<HTMLDivElement | null>(null);
  const autoScrollRef = useRef(true);

  const source = SOURCES.find((item) => item.key === activeKey) ?? SOURCES[0];

  const load = useCallback(async () => {
    setLoading(true);
    try {
      const text = await invoke<string>(source.command, { maxBytes: 160000 });
      setRawLog(text);
    } finally {
      setLoading(false);
    }
  }, [source.command]);

  useEffect(() => {
    void load();
  }, [load]);

  useEffect(() => {
    if (autoScrollRef.current && scrollRef.current) {
      scrollRef.current.scrollTop = scrollRef.current.scrollHeight;
    }
  }, [rawLog]);

  const lines = useMemo(() => rawLog.split(/\r?\n/).filter((line) => line.length > 0), [rawLog]);

  const filteredLines = useMemo(() => {
    let next = lines;
    if (activeCategories.length > 0) {
      const patterns = CATEGORY_FILTERS
        .filter((item) => activeCategories.includes(item.key))
        .map((item) => item.pattern);
      next = next.filter((line) => patterns.some((pattern) => pattern.test(line)));
    }
    if (filter.trim()) {
      const query = filter.trim().toLowerCase();
      next = next.filter((line) => line.toLowerCase().includes(query));
    }
    return next;
  }, [activeCategories, filter, lines]);

  const toggleCategory = (key: string) => {
    setActiveCategories((current) =>
      current.includes(key) ? current.filter((item) => item !== key) : [...current, key],
    );
  };

  const handleScroll = () => {
    if (!scrollRef.current) return;
    const { scrollTop, scrollHeight, clientHeight } = scrollRef.current;
    autoScrollRef.current = scrollHeight - scrollTop - clientHeight < 80;
  };

  const scrollToBottom = () => {
    if (!scrollRef.current) return;
    scrollRef.current.scrollTop = scrollRef.current.scrollHeight;
    autoScrollRef.current = true;
  };

  const copyLog = async (text: string) => {
    if (!text.trim()) {
      message.info('当前没有可复制的日志');
      return;
    }
    const ok = await copy(text);
    if (ok) message.success('日志已复制');
    else message.error('复制失败');
  };

  const openLogLocation = async () => {
    const path = await invoke<string>('get_log_file_path', { source: source.key }).catch(() => '');
    if (!path) return;
    await revealItemInDir(path);
  };

  return (
    <div style={{ height: '100%', display: 'flex', flexDirection: 'column', background: token.colorBgLayout }}>
      <div style={{ padding: '18px 24px 12px', borderBottom: `1px solid ${token.colorBorderSecondary}`, background: token.colorBgContainer }}>
        <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', gap: 16 }}>
          <Space align="center">
            <span style={{ width: 34, height: 34, display: 'inline-flex', alignItems: 'center', justifyContent: 'center', borderRadius: 8, background: token.colorPrimaryBg, color: token.colorPrimary }}>
              <ScrollText size={18} />
            </span>
            <div>
              <Typography.Title level={4} style={{ margin: 0 }}>系统日志</Typography.Title>
              <Typography.Text type="secondary">查看安装日志、Sidecar 日志和 IM 通道运行信息。</Typography.Text>
            </div>
          </Space>
          <Space wrap>
            <Button icon={<Copy size={14} />} onClick={() => void copyLog(filteredLines.join('\n'))}>
              复制
            </Button>
            <Button icon={<RefreshCw size={14} className={loading ? 'animate-spin' : ''} />} onClick={() => void load()} loading={loading}>
              刷新
            </Button>
            <Button icon={<FolderOpen size={14} />} onClick={() => void openLogLocation()}>
              打开位置
            </Button>
          </Space>
        </div>

        <div style={{ display: 'flex', alignItems: 'center', gap: 12, marginTop: 14, flexWrap: 'wrap' }}>
          <Segmented
            value={activeKey}
            options={SOURCES.map((item) => ({ label: item.label, value: item.key }))}
            onChange={(value) => setActiveKey(String(value))}
          />
          <Input
            allowClear
            value={filter}
            onChange={(event) => setFilter(event.target.value)}
            placeholder="过滤日志..."
            prefix={<Search size={14} style={{ color: token.colorTextSecondary }} />}
            style={{ width: 260 }}
          />
          <Space size={6} wrap>
            {CATEGORY_FILTERS.map((item) => (
              <Tag.CheckableTag
                key={item.key}
                checked={activeCategories.includes(item.key)}
                onChange={() => toggleCategory(item.key)}
                style={{ border: `1px solid ${activeCategories.includes(item.key) ? token.colorPrimaryBorder : token.colorBorderSecondary}` }}
              >
                {item.label}
              </Tag.CheckableTag>
            ))}
          </Space>
          <Typography.Text type="secondary" style={{ marginLeft: 'auto', fontSize: 12 }}>
            {(filter || activeCategories.length > 0) ? `${filteredLines.length} / ` : ''}{lines.length} 行 · {source.pathHint}
          </Typography.Text>
        </div>
      </div>

      <div
        ref={scrollRef}
        onScroll={handleScroll}
        style={{
          flex: 1,
          overflow: 'auto',
          background: '#0a0e17',
          padding: '14px 18px',
          fontFamily: 'var(--code-font-family, ui-monospace, SFMono-Regular, Consolas, monospace)',
          fontSize: 12,
          lineHeight: 1.7,
          userSelect: 'text',
        }}
      >
        {filteredLines.length === 0 ? (
          <Empty
            image={Empty.PRESENTED_IMAGE_SIMPLE}
            description={<span style={{ color: '#9aa4b2' }}>{lines.length === 0 ? '暂无日志' : '没有匹配的日志行'}</span>}
          />
        ) : (
          filteredLines.map((line, index) => (
            <div key={`${index}-${line.slice(0, 24)}`} style={{ whiteSpace: 'pre-wrap', wordBreak: 'break-all', color: lineColor(line, token), userSelect: 'text' }}>
              {line}
            </div>
          ))
        )}
      </div>

      {filteredLines.length > 50 && (
        <Button
          type="primary"
          icon={<ArrowDown size={14} />}
          onClick={scrollToBottom}
          style={{ position: 'absolute', right: 24, bottom: 24, borderRadius: 999 }}
        >
          到底部
        </Button>
      )}
    </div>
  );
}
