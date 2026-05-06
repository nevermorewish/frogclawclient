import { useCallback, useRef, useEffect, useState } from 'react';
import { Dropdown, Tooltip, App, theme, Spin } from 'antd';
import type { MenuProps } from 'antd';
import { Sun, Moon, Monitor, Globe, Pin, PinOff, RotateCcw, ArrowDownCircle, Minus, X, Square } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { useSettingsStore } from '@/stores';
import { isTauri, invoke } from '@/lib/invoke';
import { useUpdate } from '@/contexts/UpdateContext';
import { LANG_OPTIONS } from '@/lib/constants';
import appLogo from '@/assets/image/logo.png';

const IS_WINDOWS = navigator.userAgent.includes('Windows');

const RestoreIcon = () => (
  <svg width="14" height="14" viewBox="0 0 14 14" fill="none" stroke="currentColor" strokeWidth="1.2">
    <rect x="3" y="5" width="8" height="7" rx="0.5" />
    <path d="M5 5V3.5a.5.5 0 0 1 .5-.5H12a.5.5 0 0 1 .5.5V10a.5.5 0 0 1-.5.5h-1.5" />
  </svg>
);

const THEME_OPTIONS = [
  { key: 'system', icon: <Monitor size={14} />, labelKey: 'settings.themeSystem' },
  { key: 'light', icon: <Sun size={14} />, labelKey: 'settings.themeLight' },
  { key: 'dark', icon: <Moon size={14} />, labelKey: 'settings.themeDark' },
] as const;

const THEME_ICONS: Record<string, React.ReactNode> = {
  system: <Monitor size={14} />,
  light: <Sun size={14} />,
  dark: <Moon size={14} />,
};

export function TitleBar() {
  const { t, i18n } = useTranslation();
  const { token } = theme.useToken();
  const { modal } = App.useApp();
  const themeMode = useSettingsStore((s) => s.settings.theme_mode);
  const alwaysOnTop = useSettingsStore((s) => s.settings.always_on_top);
  const saveSettings = useSettingsStore((s) => s.saveSettings);
  const [pinned, setPinned] = useState(alwaysOnTop ?? false);
  const { checkUpdate, isChecking: checkingUpdate } = useUpdate();
  const [isMaximized, setIsMaximized] = useState(false);
  const tauriWindowRef = useRef<typeof import('@tauri-apps/api/window') | null>(null);
  const dragTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  useEffect(() => {
    setPinned(alwaysOnTop ?? false);
  }, [alwaysOnTop]);

  useEffect(() => {
    if (!IS_WINDOWS || !isTauri()) return;
    let unlisten: (() => void) | undefined;
    (async () => {
      const { getCurrentWindow } = await import('@tauri-apps/api/window');
      const win = getCurrentWindow();
      setIsMaximized(await win.isMaximized());
      unlisten = await win.onResized(async () => {
        setIsMaximized(await win.isMaximized());
      });
    })();
    return () => {
      unlisten?.();
    };
  }, []);

  useEffect(() => {
    if (isTauri()) {
      import('@tauri-apps/api/window').then((mod) => {
        tauriWindowRef.current = mod;
      });
    }
  }, []);

  const handlePinToggle = useCallback(async () => {
    const next = !pinned;
    setPinned(next);
    try {
      await invoke('set_always_on_top', { enabled: next });
      saveSettings({ always_on_top: next });
    } catch {
      setPinned(!next);
    }
  }, [pinned, saveSettings]);

  const handleCheckUpdate = useCallback(async () => {
    await checkUpdate(true);
  }, [checkUpdate]);

  const themeMenuItems: MenuProps['items'] = THEME_OPTIONS.map((opt) => ({
    key: opt.key,
    icon: opt.icon,
    label: t(opt.labelKey),
  }));

  const langMenuItems: MenuProps['items'] = LANG_OPTIONS.map((opt) => ({
    key: opt.key,
    icon: <span>{opt.icon}</span>,
    label: opt.label,
  }));

  const handleThemeChange: MenuProps['onClick'] = ({ key }) => {
    saveSettings({ theme_mode: key });
  };

  const handleLangChange: MenuProps['onClick'] = ({ key }) => {
    i18n.changeLanguage(key);
    saveSettings({ language: key });
  };

  const handleReload = useCallback(() => {
    modal.confirm({
      title: t('desktop.reloadConfirmTitle'),
      content: t('desktop.reloadConfirmContent'),
      okText: t('desktop.reloadConfirmOk'),
      cancelText: t('desktop.reloadConfirmCancel'),
      onOk: () => {
        window.location.reload();
      },
    });
  }, [modal, t]);

  const handleWindowMinimize = useCallback(async () => {
    await invoke('minimize_window');
  }, []);

  const handleWindowMaximize = useCallback(async () => {
    await invoke('toggle_maximize_window');
  }, []);

  const handleWindowClose = useCallback(async () => {
    const { getCurrentWindow } = await import('@tauri-apps/api/window');
    await getCurrentWindow().close();
  }, []);

  const handleDragMouseDown = useCallback((e: React.MouseEvent<HTMLDivElement>) => {
    const target = e.target as HTMLElement;
    if (target.closest('button')) return;
    const mod = tauriWindowRef.current;
    if (!mod) return;
    e.preventDefault();

    if (IS_WINDOWS) {
      if (dragTimerRef.current) clearTimeout(dragTimerRef.current);
      dragTimerRef.current = setTimeout(() => {
        mod.getCurrentWindow().startDragging();
      }, 200);
    } else {
      mod.getCurrentWindow().startDragging();
    }
  }, []);

  const handleTitleBarDoubleClick = useCallback(() => {
    if (!IS_WINDOWS) return;
    if (dragTimerRef.current) {
      clearTimeout(dragTimerRef.current);
      dragTimerRef.current = null;
    }
    invoke('toggle_maximize_window');
  }, []);

  const buttonBase: React.CSSProperties = {
    width: 28,
    height: 28,
    display: 'flex',
    alignItems: 'center',
    justifyContent: 'center',
    borderRadius: token.borderRadius,
    fontSize: 14,
    cursor: 'pointer',
    border: 'none',
    backgroundColor: 'transparent',
  };

  const hoverHandlers = (baseColor: string) => ({
    onMouseEnter: (e: React.MouseEvent<HTMLButtonElement>) => {
      e.currentTarget.style.backgroundColor = token.colorFillSecondary;
      e.currentTarget.style.color = token.colorTextBase;
    },
    onMouseLeave: (e: React.MouseEvent<HTMLButtonElement>) => {
      e.currentTarget.style.backgroundColor = 'transparent';
      e.currentTarget.style.color = baseColor;
    },
  });

  const appName = t('app.name');

  return (
    <div
      className="title-bar-drag"
      {...(!IS_WINDOWS ? { 'data-tauri-drag-region': true } : {})}
      onMouseDown={handleDragMouseDown}
      onDoubleClick={IS_WINDOWS ? handleTitleBarDoubleClick : undefined}
      style={{
        height: 36,
        display: 'flex',
        alignItems: 'center',
        justifyContent: 'space-between',
        paddingLeft: IS_WINDOWS ? 12 : 72,
        paddingRight: IS_WINDOWS ? 0 : 12,
        backgroundColor: 'transparent',
        flexShrink: 0,
        borderBottom: `1px solid ${token.colorBorderSecondary}`,
      }}
    >
      {IS_WINDOWS ? (
        <div className="title-bar-nodrag" style={{ display: 'flex', alignItems: 'center', gap: 6, marginRight: 8 }}>
          <img src={appLogo} alt={appName} style={{ width: 18, height: 18 }} draggable={false} />
          <span style={{ fontSize: 13, fontWeight: 600, color: token.colorTextBase, userSelect: 'none' }}>{appName}</span>
        </div>
      ) : (
        <div />
      )}

      <div style={{ display: 'flex', alignItems: 'center', gap: 0 }}>
        <div className="title-bar-nodrag" style={{ display: 'flex', alignItems: 'center', gap: 4 }}>
          <Tooltip title={t('desktop.alwaysOnTop')}>
            <button
              onClick={handlePinToggle}
              style={{
                ...buttonBase,
                color: pinned ? token.colorPrimary : token.colorTextSecondary,
              }}
              onMouseEnter={(e) => {
                e.currentTarget.style.backgroundColor = pinned ? token.colorPrimaryBg : token.colorFillSecondary;
                e.currentTarget.style.color = pinned ? token.colorPrimary : token.colorTextBase;
              }}
              onMouseLeave={(e) => {
                e.currentTarget.style.backgroundColor = 'transparent';
                e.currentTarget.style.color = pinned ? token.colorPrimary : token.colorTextSecondary;
              }}
            >
              {pinned ? <Pin size={14} /> : <PinOff size={14} />}
            </button>
          </Tooltip>

          <Dropdown
            menu={{ items: themeMenuItems, onClick: handleThemeChange, selectedKeys: [themeMode] }}
            trigger={['click']}
            placement="bottomRight"
            destroyOnHidden
          >
            <button style={{ ...buttonBase, color: token.colorTextSecondary }} {...hoverHandlers(token.colorTextSecondary)}>
              {THEME_ICONS[themeMode] ?? <Monitor size={14} />}
            </button>
          </Dropdown>

          <Dropdown
            menu={{ items: langMenuItems, onClick: handleLangChange, selectedKeys: [i18n.language] }}
            trigger={['click']}
            placement="bottomRight"
            destroyOnHidden
          >
            <button style={{ ...buttonBase, color: token.colorTextSecondary }} {...hoverHandlers(token.colorTextSecondary)}>
              <Globe size={14} />
            </button>
          </Dropdown>

          {isTauri() && (
            <Tooltip title={t('settings.checkUpdate')}>
              <button
                onClick={handleCheckUpdate}
                disabled={checkingUpdate}
                style={{ ...buttonBase, color: token.colorTextSecondary, opacity: checkingUpdate ? 0.5 : 1 }}
                {...hoverHandlers(token.colorTextSecondary)}
              >
                {checkingUpdate ? <Spin size="small" /> : <ArrowDownCircle size={14} />}
              </button>
            </Tooltip>
          )}

          <Tooltip title={t('desktop.reloadPage')}>
            <button
              onClick={handleReload}
              style={{ ...buttonBase, color: token.colorTextSecondary }}
              {...hoverHandlers(token.colorTextSecondary)}
            >
              <RotateCcw size={14} />
            </button>
          </Tooltip>
        </div>

        {IS_WINDOWS && isTauri() && (
          <div className="title-bar-nodrag" style={{ display: 'flex', alignItems: 'center', marginLeft: 4 }}>
            <button
              onClick={handleWindowMinimize}
              style={{
                display: 'flex',
                alignItems: 'center',
                justifyContent: 'center',
                width: 46,
                height: 36,
                border: 'none',
                background: 'transparent',
                color: token.colorTextSecondary,
                cursor: 'pointer',
                outline: 'none',
              }}
              onMouseEnter={(e) => {
                e.currentTarget.style.backgroundColor = token.colorFillSecondary;
                e.currentTarget.style.color = token.colorTextBase;
              }}
              onMouseLeave={(e) => {
                e.currentTarget.style.backgroundColor = 'transparent';
                e.currentTarget.style.color = token.colorTextSecondary;
              }}
            >
              <Minus size={16} />
            </button>
            <button
              onClick={handleWindowMaximize}
              style={{
                display: 'flex',
                alignItems: 'center',
                justifyContent: 'center',
                width: 46,
                height: 36,
                border: 'none',
                background: 'transparent',
                color: token.colorTextSecondary,
                cursor: 'pointer',
                outline: 'none',
              }}
              onMouseEnter={(e) => {
                e.currentTarget.style.backgroundColor = token.colorFillSecondary;
                e.currentTarget.style.color = token.colorTextBase;
              }}
              onMouseLeave={(e) => {
                e.currentTarget.style.backgroundColor = 'transparent';
                e.currentTarget.style.color = token.colorTextSecondary;
              }}
            >
              {isMaximized ? <RestoreIcon /> : <Square size={14} />}
            </button>
            <button
              onClick={handleWindowClose}
              style={{
                display: 'flex',
                alignItems: 'center',
                justifyContent: 'center',
                width: 46,
                height: 36,
                border: 'none',
                background: 'transparent',
                color: token.colorTextSecondary,
                cursor: 'pointer',
                outline: 'none',
                borderRadius: 0,
              }}
              onMouseEnter={(e) => {
                e.currentTarget.style.backgroundColor = '#e81123';
                e.currentTarget.style.color = '#ffffff';
              }}
              onMouseLeave={(e) => {
                e.currentTarget.style.backgroundColor = 'transparent';
                e.currentTarget.style.color = token.colorTextSecondary;
              }}
            >
              <X size={16} />
            </button>
          </div>
        )}
      </div>
    </div>
  );
}
