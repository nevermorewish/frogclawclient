import { useCallback, useRef, useState } from 'react';
import { checkForUpdate, getCurrentVersion } from '@/lib/updater';
import type { CheckResult } from '@/lib/updater';

export function useUpdateCheck() {
  const [isChecking, setIsChecking] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [lastChecked, setLastChecked] = useState<Date | null>(null);
  const isCheckingRef = useRef(false);

  const checkUpdate = useCallback(async (force = false): Promise<CheckResult> => {
    if (isCheckingRef.current) {
      return { status: 'error', error: 'Update check already in progress' };
    }

    if (!force && lastChecked && Date.now() - lastChecked.getTime() < 5 * 60 * 1000) {
      return {
        status: 'up-to-date',
        currentVersion: await getCurrentVersion(),
        skipped: true,
      };
    }

    isCheckingRef.current = true;
    setIsChecking(true);
    setError(null);

    try {
      const result = await checkForUpdate();
      setLastChecked(new Date());
      setError(result.status === 'error' ? result.error : null);
      return result;
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err ?? 'Unknown update check error');
      setError(message);
      return { status: 'error', error: message };
    } finally {
      isCheckingRef.current = false;
      setIsChecking(false);
    }
  }, [lastChecked]);

  return { isChecking, error, lastChecked, checkUpdate };
}
