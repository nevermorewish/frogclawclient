import { create } from 'zustand';
import { applyFrogclawTokenSelection, fetchAndConfigureFrogclaw } from '@/lib/homeApi';
import type { FrogclawConfigureResult, FrogclawToken, FrogclawUserData } from '@/types';

const STORAGE_KEY = 'frogclaw_home_last_result';

function parseStoredResult(): FrogclawConfigureResult | null {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (!raw) return null;
    const parsed = JSON.parse(raw) as FrogclawConfigureResult;
    if (!parsed?.session?.user || !Array.isArray(parsed.session.tokens)) {
      localStorage.removeItem(STORAGE_KEY);
      return null;
    }
    if (!Array.isArray(parsed.auto_cli_configs)) {
      parsed.auto_cli_configs = [];
    }
    return parsed;
  } catch {
    localStorage.removeItem(STORAGE_KEY);
    return null;
  }
}

interface FrogclawAuthState {
  result: FrogclawConfigureResult | null;
  selectedTokenId: number | null;
  login: (username: string, password: string) => Promise<FrogclawConfigureResult>;
  logout: () => void;
  selectToken: (tokenId: number) => Promise<FrogclawConfigureResult | null>;
}

const initialResult = parseStoredResult();

export const useFrogclawAuthStore = create<FrogclawAuthState>((set, get) => ({
  result: initialResult,
  selectedTokenId: initialResult?.selected_token_id ?? null,
  login: async (username, password) => {
    const configureResult = await fetchAndConfigureFrogclaw(username, password, get().selectedTokenId);
    localStorage.setItem(STORAGE_KEY, JSON.stringify(configureResult));
    set({
      result: configureResult,
      selectedTokenId: configureResult.selected_token_id,
    });
    return configureResult;
  },
  logout: () => {
    localStorage.removeItem(STORAGE_KEY);
    set({ result: null, selectedTokenId: null });
  },
  selectToken: async (tokenId) => {
    const current = get().result;
    if (!current) return null;
    const configuredProviders = await applyFrogclawTokenSelection(current.session, tokenId);
    const nextResult: FrogclawConfigureResult = {
      ...current,
      selected_token_id: tokenId,
      configured_providers: configuredProviders,
    };
    localStorage.setItem(STORAGE_KEY, JSON.stringify(nextResult));
    set({ result: nextResult, selectedTokenId: tokenId });
    return nextResult;
  },
}));

export function useFrogclawUser(): FrogclawUserData | null {
  return useFrogclawAuthStore((state) => state.result?.session.user ?? null);
}

export function useFrogclawTokens(): FrogclawToken[] {
  return useFrogclawAuthStore((state) => state.result?.session.tokens ?? []);
}
