import { getVersion } from '@tauri-apps/api/app';
import type { Update } from '@tauri-apps/plugin-updater';
import { invoke, isTauri } from '@/lib/invoke';

export interface UpdateInfo {
  currentVersion: string;
  availableVersion: string;
  body?: string;
  date?: string;
}

export interface UpdateProgressEvent {
  event: 'Started' | 'Progress' | 'Finished';
  total?: number;
  downloaded?: number;
}

export interface UpdateHandle {
  version: string;
  body?: string;
  date?: string;
  close: () => Promise<void>;
  downloadAndInstall: (onProgress?: (event: UpdateProgressEvent) => void) => Promise<void>;
}

export type CheckResult =
  | { status: 'up-to-date'; currentVersion: string; skipped?: boolean }
  | { status: 'available'; info: UpdateInfo; update: UpdateHandle }
  | { status: 'error'; error: string };

function writeUpdaterLog(level: string, message: string) {
  if (!isTauri()) return;
  invoke('platform_write_log', { level, message: `[updater] ${message}` }).catch(() => {});
}

function mapUpdateHandle(update: Update): UpdateHandle {
  return {
    version: update.version,
    body: update.body,
    date: update.date,
    close: () => update.close(),
    async downloadAndInstall(onProgress) {
      let downloaded = 0;
      await update.downloadAndInstall((event) => {
        if (!onProgress) return;
        if (event.event === 'Started') {
          downloaded = 0;
          onProgress({
            event: event.event,
            total: event.data.contentLength ?? 0,
            downloaded,
          });
        } else if (event.event === 'Progress') {
          downloaded += event.data.chunkLength;
          onProgress({
            event: event.event,
            downloaded,
          });
        } else {
          onProgress({ event: event.event, downloaded });
        }
      });
    },
  };
}

function friendlyUpdateError(error: unknown) {
  const raw = error instanceof Error ? error.message : String(error ?? '');
  const lower = raw.toLowerCase();

  if (raw.includes('404') || lower.includes('not found')) {
    return 'Update metadata was not found. Check the updater endpoint and release assets.';
  }
  if (lower.includes('timeout') || lower.includes('network') || lower.includes('connection')) {
    return 'Update check timed out. Check the network connection and try again.';
  }
  if (lower.includes('signature') || lower.includes('verify') || lower.includes('minisign')) {
    return 'Update signature verification failed. Check the signing key and release signatures.';
  }
  if (lower.includes('permission') || lower.includes('not allowed')) {
    return 'The app does not have updater permission enabled.';
  }
  return raw || 'Failed to check for updates.';
}

export async function getCurrentVersion() {
  try {
    return await getVersion();
  } catch {
    return '0.0.0';
  }
}

export async function checkForUpdate(timeout = 30000): Promise<CheckResult> {
  if (!isTauri()) {
    return { status: 'up-to-date', currentVersion: '0.0.0' };
  }

  try {
    writeUpdaterLog('info', `Starting update check, timeout=${timeout}ms`);
    const [{ check }, currentVersion] = await Promise.all([
      import('@tauri-apps/plugin-updater'),
      getCurrentVersion(),
    ]);
    const update = await check({ timeout });

    if (!update) {
      writeUpdaterLog('info', `No update available, current=${currentVersion}`);
      return { status: 'up-to-date', currentVersion };
    }

    const handle = mapUpdateHandle(update);
    writeUpdaterLog('info', `Update available, current=${currentVersion}, latest=${handle.version}`);
    return {
      status: 'available',
      info: {
        currentVersion,
        availableVersion: handle.version,
        body: handle.body,
        date: handle.date,
      },
      update: handle,
    };
  } catch (error) {
    const message = friendlyUpdateError(error);
    writeUpdaterLog('error', `Update check failed: ${message}`);
    return { status: 'error', error: message };
  }
}

export async function relaunchApp() {
  const { relaunch } = await import('@tauri-apps/plugin-process');
  await relaunch();
}
