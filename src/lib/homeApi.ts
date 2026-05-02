import { invoke } from './invoke';
import type {
  FrogclawConfigureResult,
  FrogclawConfiguredProvider,
  FrogclawLoginSession,
  HomeToolsStatus,
  InstallResult,
} from '@/types';

export async function checkToolsInstalled(): Promise<HomeToolsStatus> {
  return invoke<HomeToolsStatus>('check_tools_installed');
}

export async function installTool(toolId: string): Promise<InstallResult> {
  return invoke<InstallResult>('install_tool', { toolId });
}

export async function fetchAndConfigureFrogclaw(
  username: string,
  password: string,
  selectedTokenId?: number | null,
): Promise<FrogclawConfigureResult> {
  return invoke<FrogclawConfigureResult>('fetch_and_configure_frogclaw', {
    username,
    password,
    selectedTokenId,
  });
}

export async function applyFrogclawTokenSelection(
  session: FrogclawLoginSession,
  selectedTokenId: number,
): Promise<FrogclawConfiguredProvider[]> {
  return invoke<FrogclawConfiguredProvider[]>('apply_frogclaw_token_selection', {
    session,
    selectedTokenId,
  });
}
