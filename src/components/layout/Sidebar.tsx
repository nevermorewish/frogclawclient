import { useState } from 'react';
import { Tooltip, Avatar, theme } from 'antd';
import { MessageSquare, Brain, FolderOpen, User, Sparkles, ImagePlus, MessageSquarePlus, Search, Settings, XCircle, Home } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { useUIStore } from '@/stores';
import { useUserProfileStore } from '@/stores/userProfileStore';
import { useResolvedAvatarSrc } from '@/hooks/useResolvedAvatarSrc';
import { UserProfileModal } from './UserProfileModal';
import type { PageKey } from '@/types';

const mainNavItems: { key: PageKey; icon: React.ReactNode; labelKey: string }[] = [
  { key: 'home', icon: <Home size={18} />, labelKey: 'nav.home' },
  { key: 'chat', icon: <MessageSquare size={18} />, labelKey: 'nav.chat' },
  { key: 'drawing', icon: <ImagePlus size={18} />, labelKey: 'nav.drawing' },
  { key: 'skills', icon: <Sparkles size={18} />, labelKey: 'nav.skills' },
  { key: 'memory', icon: <Brain size={18} />, labelKey: 'nav.memory' },
  { key: 'files', icon: <FolderOpen size={18} />, labelKey: 'nav.files' },
];

export function Sidebar() {
  const { t } = useTranslation();
  const { token } = theme.useToken();
  const activePage = useUIStore((s) => s.activePage);
  const setActivePage = useUIStore((s) => s.setActivePage);
  const enterSettings = useUIStore((s) => s.enterSettings);
  const exitSettings = useUIStore((s) => s.exitSettings);
  const profile = useUserProfileStore((s) => s.profile);
  const [profileModalOpen, setProfileModalOpen] = useState(false);
  const resolvedAvatarSrc = useResolvedAvatarSrc(profile.avatarType, profile.avatarValue);

  const dispatchChatAction = (eventName: string) => {
    const shouldDelayDispatch = activePage !== 'chat';
    setActivePage('chat');
    window.setTimeout(() => {
      window.dispatchEvent(new CustomEvent(eventName));
    }, shouldDelayDispatch ? 80 : 0);
  };

  const handleSettingsToggle = () => {
    if (activePage === 'settings') {
      exitSettings();
    } else {
      enterSettings();
    }
  };

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
            height: 36,
            borderRadius: token.borderRadius,
            backgroundColor: isActive ? token.colorPrimaryBg : 'transparent',
            color: isActive ? token.colorPrimary : token.colorTextSecondary,
            border: 'none',
            padding: '0 12px',
            gap: 10,
            fontWeight: isActive ? 600 : 500,
            textAlign: 'left',
            cursor: 'pointer',
          }}
          onMouseEnter={(e) => {
            if (!isActive) {
              e.currentTarget.style.backgroundColor = token.colorFillSecondary;
              e.currentTarget.style.color = token.colorTextBase;
            }
          }}
          onMouseLeave={(e) => {
            if (!isActive) {
              e.currentTarget.style.backgroundColor = 'transparent';
              e.currentTarget.style.color = token.colorTextSecondary;
            }
          }}
        >
          <span style={{ display: 'inline-flex', width: 20, justifyContent: 'center', flexShrink: 0 }}>
            {item.icon}
          </span>
          <span className="truncate" style={{ fontSize: 13 }}>
            {label}
          </span>
        </button>
      </Tooltip>
    );
  };

  const renderActionButton = (
    key: string,
    icon: React.ReactNode,
    label: string,
    eventName: string,
    primary = false,
  ) => (
    <Tooltip key={key} title={label} placement="right">
      <button
        onClick={() => dispatchChatAction(eventName)}
        className="flex items-center transition-colors"
        style={{
          width: '100%',
          height: 34,
          borderRadius: token.borderRadius,
          border: `1px solid ${primary ? token.colorPrimaryBorder : token.colorBorderSecondary}`,
          backgroundColor: primary ? token.colorPrimaryBg : 'transparent',
          color: primary ? token.colorPrimary : token.colorTextSecondary,
          padding: '0 12px',
          gap: 10,
          fontWeight: 600,
          cursor: 'pointer',
        }}
        onMouseEnter={(e) => {
          e.currentTarget.style.backgroundColor = primary ? token.colorPrimaryBgHover : token.colorFillSecondary;
          e.currentTarget.style.color = primary ? token.colorPrimary : token.colorTextBase;
        }}
        onMouseLeave={(e) => {
          e.currentTarget.style.backgroundColor = primary ? token.colorPrimaryBg : 'transparent';
          e.currentTarget.style.color = primary ? token.colorPrimary : token.colorTextSecondary;
        }}
      >
        <span style={{ display: 'inline-flex', width: 20, justifyContent: 'center', flexShrink: 0 }}>
          {icon}
        </span>
        <span className="truncate" style={{ fontSize: 13 }}>
          {label}
        </span>
      </button>
    </Tooltip>
  );

  const renderUserAvatar = () => {
    const size = 32;
    if (profile.avatarType === 'emoji' && profile.avatarValue) {
      return (
        <div
          style={{
            width: size,
            height: size,
            borderRadius: '50%',
            backgroundColor: token.colorFillSecondary,
            display: 'flex',
            alignItems: 'center',
            justifyContent: 'center',
            fontSize: 16,
            cursor: 'pointer',
          }}
        >
          {profile.avatarValue}
        </div>
      );
    }
    if ((profile.avatarType === 'url' || profile.avatarType === 'file') && profile.avatarValue) {
      const src = profile.avatarType === 'file' ? resolvedAvatarSrc : profile.avatarValue;
      return <Avatar size={size} src={src} style={{ cursor: 'pointer' }} />;
    }
    return (
      <Avatar
        size={size}
        icon={<User size={16} />}
        style={{ cursor: 'pointer', backgroundColor: token.colorPrimary }}
      />
    );
  };

  return (
    <div className="flex flex-col h-full" style={{ padding: '10px 10px 12px' }}>
      <div className="flex flex-col gap-2" style={{ marginBottom: 12 }}>
        {renderActionButton(
          'new-conversation',
          <MessageSquarePlus size={17} />,
          t('chat.newConversation'),
          'frogclaw:new-conversation',
          true,
        )}
        {renderActionButton(
          'search-conversations',
          <Search size={17} />,
          t('commandPalette.searchConversations'),
          'frogclaw:focus-conversation-search',
        )}
      </div>

      <nav className="flex flex-col gap-1">
        {mainNavItems.map(renderNavButton)}
      </nav>

      <div className="flex-1" />

      {/* User Avatar */}
      <Tooltip title={profile.name || t('userProfile.title')} placement="right">
        <button
          onClick={() => setProfileModalOpen(true)}
          className="flex items-center"
          style={{
            width: '100%',
            height: 38,
            background: 'none',
            border: 'none',
            padding: '0 8px',
            gap: 10,
            color: token.colorTextSecondary,
            cursor: 'pointer',
          }}
        >
          {renderUserAvatar()}
          <span className="truncate" style={{ fontSize: 13, fontWeight: 500 }}>
            {profile.name || t('userProfile.title')}
          </span>
        </button>
      </Tooltip>

      <Tooltip
        title={activePage === 'settings' ? t('settings.closeSettings') : t('settings.openSettings')}
        placement="right"
      >
        <button
          onClick={handleSettingsToggle}
          className="flex items-center transition-colors"
          style={{
            width: '100%',
            height: 36,
            borderRadius: token.borderRadius,
            backgroundColor: activePage === 'settings' ? token.colorPrimaryBg : 'transparent',
            color: activePage === 'settings' ? token.colorPrimary : token.colorTextSecondary,
            border: 'none',
            padding: '0 12px',
            gap: 10,
            fontWeight: activePage === 'settings' ? 600 : 500,
            textAlign: 'left',
            cursor: 'pointer',
          }}
          onMouseEnter={(e) => {
            if (activePage !== 'settings') {
              e.currentTarget.style.backgroundColor = token.colorFillSecondary;
              e.currentTarget.style.color = token.colorTextBase;
            }
          }}
          onMouseLeave={(e) => {
            if (activePage !== 'settings') {
              e.currentTarget.style.backgroundColor = 'transparent';
              e.currentTarget.style.color = token.colorTextSecondary;
            }
          }}
        >
          <span style={{ display: 'inline-flex', width: 20, justifyContent: 'center', flexShrink: 0 }}>
            {activePage === 'settings' ? <XCircle size={18} /> : <Settings size={18} />}
          </span>
          <span className="truncate" style={{ fontSize: 13 }}>
            {activePage === 'settings' ? t('settings.closeSettings') : t('settings.openSettings')}
          </span>
        </button>
      </Tooltip>

      <UserProfileModal open={profileModalOpen} onClose={() => setProfileModalOpen(false)} />
    </div>
  );
}
