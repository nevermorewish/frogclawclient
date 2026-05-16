import { useState } from 'react';
import { Alert, Button, Modal, Progress, Typography } from 'antd';
import { Download, RefreshCw } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { useUpdate } from '@/contexts/UpdateContext';
import { relaunchApp } from '@/lib/updater';

const { Text } = Typography;

export function UpdateDialog() {
  const { t } = useTranslation();
  const { dialogOpen, setDialogOpen, updateInfo, updateHandle, dismissUpdate } = useUpdate();
  const [isInstalling, setIsInstalling] = useState(false);
  const [isInstalled, setIsInstalled] = useState(false);
  const [progress, setProgress] = useState(0);
  const [error, setError] = useState<string | null>(null);

  if (!updateInfo) return null;

  const installUpdate = async () => {
    if (!updateHandle) {
      setError('Automatic update is not available for this build.');
      return;
    }

    setIsInstalling(true);
    setIsInstalled(false);
    setProgress(0);
    setError(null);

    try {
      let total = 0;
      await updateHandle.downloadAndInstall((event) => {
        if (event.event === 'Started') {
          total = event.total ?? 0;
          setProgress(0);
        } else if (event.event === 'Progress' && total > 0) {
          setProgress(Math.min(99, Math.round(((event.downloaded ?? 0) / total) * 100)));
        } else if (event.event === 'Finished') {
          setProgress(100);
        }
      });
      setProgress(100);
      setIsInstalled(true);
    } catch (err) {
      const raw = err instanceof Error ? err.message : String(err ?? '');
      const lower = raw.toLowerCase();
      if (lower.includes('signature') || lower.includes('verify') || lower.includes('minisign')) {
        setError(`Update signature verification failed.\n${raw}`);
      } else {
        setError(`${t('settings.updateFailed')}.\n${raw}`);
      }
    } finally {
      setIsInstalling(false);
    }
  };

  const restart = async () => {
    try {
      await relaunchApp();
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err ?? 'Failed to relaunch app'));
    }
  };

  return (
    <Modal
      title={t('settings.updateAvailable')}
      open={dialogOpen}
      centered
      maskClosable={!isInstalling}
      keyboard={!isInstalling}
      onCancel={() => !isInstalling && setDialogOpen(false)}
      footer={[
        <Button key="later" disabled={isInstalling} onClick={dismissUpdate}>
          {t('settings.updateLater')}
        </Button>,
        isInstalled ? (
          <Button key="restart" type="primary" icon={<RefreshCw size={16} />} onClick={restart}>
            Restart
          </Button>
        ) : (
          <Button key="update" type="primary" icon={<Download size={16} />} loading={isInstalling} onClick={installUpdate}>
            {t('settings.updateNow')}
          </Button>
        ),
      ]}
    >
      <div style={{ display: 'flex', flexDirection: 'column', gap: 12 }}>
        <div style={{ display: 'flex', gap: 8, flexWrap: 'wrap' }}>
          <Text type="secondary">{t('settings.version')}:</Text>
          <Text code>{updateInfo.currentVersion}</Text>
          <Text type="secondary">{t('settings.newVersion')}:</Text>
          <Text code strong>{updateInfo.availableVersion}</Text>
        </div>

        {updateInfo.body && (
          <pre style={{ maxHeight: 220, overflow: 'auto', whiteSpace: 'pre-wrap', margin: 0 }}>
            {updateInfo.body}
          </pre>
        )}

        {isInstalling && <Progress percent={progress} status="active" />}
        {isInstalled && <Alert type="success" showIcon message="Update installed. Restart to use the new version." />}
        {error && <Alert type="error" showIcon message={error} />}
      </div>
    </Modal>
  );
}
