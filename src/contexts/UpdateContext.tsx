import { createContext, useCallback, useContext, useEffect, useMemo, useState } from 'react';
import { message } from 'antd';
import { useTranslation } from 'react-i18next';
import { isTauri } from '@/lib/invoke';
import { useSettingsStore } from '@/stores';
import { useUpdateCheck } from '@/hooks/useUpdateCheck';
import type { UpdateHandle, UpdateInfo } from '@/lib/updater';

type UpdateContextValue = {
  hasUpdate: boolean;
  updateInfo: UpdateInfo | null;
  updateHandle: UpdateHandle | null;
  isChecking: boolean;
  error: string | null;
  lastChecked: Date | null;
  isDismissed: boolean;
  dialogOpen: boolean;
  setDialogOpen: (open: boolean) => void;
  checkUpdate: (force?: boolean) => Promise<boolean>;
  dismissUpdate: () => void;
  resetDismiss: () => void;
};

const DISMISSED_VERSION_KEY = 'frogclaw:update:dismissedVersion';
const UpdateContext = createContext<UpdateContextValue | undefined>(undefined);

export function UpdateProvider({ children }: { children: React.ReactNode }) {
  const { t } = useTranslation();
  const { isChecking, error, lastChecked, checkUpdate: performCheck } = useUpdateCheck();
  const updateCheckInterval = useSettingsStore((s) => s.settings.update_check_interval ?? 60);
  const [hasUpdate, setHasUpdate] = useState(false);
  const [updateInfo, setUpdateInfo] = useState<UpdateInfo | null>(null);
  const [updateHandle, setUpdateHandle] = useState<UpdateHandle | null>(null);
  const [isDismissed, setIsDismissed] = useState(false);
  const [dialogOpen, setDialogOpen] = useState(false);

  useEffect(() => {
    const version = updateInfo?.availableVersion;
    if (!version) return;
    setIsDismissed(localStorage.getItem(DISMISSED_VERSION_KEY) === version);
  }, [updateInfo?.availableVersion]);

  const checkUpdate = useCallback(async (force = false) => {
    const result = await performCheck(force);

    if (result.status === 'available') {
      setHasUpdate(true);
      setUpdateInfo(result.info);
      setUpdateHandle(result.update);

      const dismissedVersion = localStorage.getItem(DISMISSED_VERSION_KEY);
      const dismissed = !force && dismissedVersion === result.info.availableVersion;
      setIsDismissed(dismissed);
      if (!dismissed) setDialogOpen(true);
      return true;
    }

    if (result.status === 'up-to-date') {
      if (!result.skipped) {
        setHasUpdate(false);
        setUpdateInfo(null);
        setUpdateHandle(null);
        setIsDismissed(false);
      }
      if (force) message.success(t('settings.noUpdate'));
      return false;
    }

    if (force) message.error(`${t('settings.checkUpdateFailed')}: ${result.error}`);
    return false;
  }, [performCheck, t]);

  const dismissUpdate = useCallback(() => {
    setIsDismissed(true);
    setDialogOpen(false);
    if (updateInfo?.availableVersion) {
      localStorage.setItem(DISMISSED_VERSION_KEY, updateInfo.availableVersion);
    }
  }, [updateInfo?.availableVersion]);

  const resetDismiss = useCallback(() => {
    setIsDismissed(false);
    localStorage.removeItem(DISMISSED_VERSION_KEY);
  }, []);

  useEffect(() => {
    if (!isTauri()) return;
    const timer = setTimeout(() => {
      void checkUpdate(false);
    }, 3000);
    return () => clearTimeout(timer);
  }, [checkUpdate]);

  useEffect(() => {
    if (!isTauri() || !updateCheckInterval) return;
    const intervalMs = Math.max(updateCheckInterval, 1) * 60 * 1000;
    const timer = setInterval(() => {
      void checkUpdate(false);
    }, intervalMs);
    return () => clearInterval(timer);
  }, [checkUpdate, updateCheckInterval]);

  const value = useMemo<UpdateContextValue>(() => ({
    hasUpdate,
    updateInfo,
    updateHandle,
    isChecking,
    error,
    lastChecked,
    isDismissed,
    dialogOpen,
    setDialogOpen,
    checkUpdate,
    dismissUpdate,
    resetDismiss,
  }), [
    hasUpdate,
    updateInfo,
    updateHandle,
    isChecking,
    error,
    lastChecked,
    isDismissed,
    dialogOpen,
    checkUpdate,
    dismissUpdate,
    resetDismiss,
  ]);

  return <UpdateContext.Provider value={value}>{children}</UpdateContext.Provider>;
}

export function useUpdate() {
  const context = useContext(UpdateContext);
  if (!context) {
    throw new Error('useUpdate must be used within UpdateProvider');
  }
  return context;
}
