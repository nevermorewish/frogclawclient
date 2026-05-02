export type BackupJobKind = 'backup' | 'restore' | 'indexing';
export type BackupJobStatus = 'pending' | 'running' | 'success' | 'failed' | 'cancelled';
export type BackupTargetKind = 'local' | 'webdav' | 's3';

export type WebDavConfig = {
  host: string;
  username: string;
  password: string;
  path: string;
  acceptInvalidCerts: boolean;
};

export type WebDavFileInfo = {
  fileName: string;
  size: number;
  lastModified: string;
  hostname: string;
};

export type BackupManifest = {
  id: string;
  version: string;
  createdAt: string;
  encrypted: boolean;
  checksum: string;
  objectCountsJson: string;
  sourceAppVersion: string;
  filePath: string | null;
  fileSize: number;
};

export type AutoBackupSettings = {
  enabled: boolean;
  intervalHours: number;
  maxCount: number;
  backupDir: string | null;
};

export type BackupJob = {
  id: string;
  kind: BackupJobKind;
  status: BackupJobStatus;
  progress: number;
  message?: string;
  createdAt: string;
  updatedAt: string;
};

export type BackupTarget = {
  kind: BackupTargetKind;
  configJson: string;
};

export type CreateBackupJobInput = {
  target: BackupTarget;
  includeAttachments: boolean;
  includeKnowledgeFiles: boolean;
  passphrase?: string;
};

export type ProgramPolicy = {
  id: string;
  programName: string;
  allowedProviderIds: string[];
  allowedModelIds: string[];
  defaultProviderId?: string;
  defaultModelId?: string;
  rateLimitPerMinute?: number;
};

export type DesktopCapabilityKey = 'tray' | 'global_shortcut' | 'protocol_handler' | 'mini_window' | 'artifact_window' | 'notification';

export type DesktopCapability = {
  key: DesktopCapabilityKey;
  supported: boolean;
  reason?: string;
};

export type TrayAction = 'show_main' | 'open_mini_window' | 'resume_voice_call' | 'run_quick_backup' | 'quit';

export type ProtocolLaunchPayload = {
  source: 'browser' | 'os_protocol';
  route: 'chat' | 'settings';
  query?: Record<string, string>;
};

export type WindowStateSnapshot = {
  windowKey: 'main' | 'mini' | 'voice' | 'artifact';
  width: number;
  height: number;
  x?: number;
  y?: number;
  maximized: boolean;
  visible: boolean;
};

export type DesktopNotification = {
  id: string;
  level: 'info' | 'success' | 'warning' | 'error';
  title: string;
  body: string;
  actionLabel?: string;
};
