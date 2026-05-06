import { useUpdate } from '@/contexts/UpdateContext';

/**
 * Shared hook for checking app updates.
 * Used by TitleBar, App.tsx, and AboutPage to avoid duplicated logic.
 */
export function useUpdateChecker() {
  const { checkUpdate } = useUpdate();
  const checkForUpdate = (options?: { silent?: boolean }) => checkUpdate(!options?.silent);

  return { checkForUpdate };
}
