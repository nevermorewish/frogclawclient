import { invoke } from './invoke';
import type {
  FrogclawConfigureResult,
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
): Promise<FrogclawConfigureResult> {
  return invoke<FrogclawConfigureResult>('fetch_and_configure_frogclaw', {
    username,
    password,
  });
}
