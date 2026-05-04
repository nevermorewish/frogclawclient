import { create } from 'zustand';
import { invoke } from '@/lib/invoke';
import type { MemoryNamespace, MemoryItem, ProjectMemoryProfile, UpdateMemoryNamespaceInput, UpdateMemoryItemInput } from '@/types';

interface MemoryState {
  namespaces: MemoryNamespace[];
  projectProfiles: ProjectMemoryProfile[];
  items: MemoryItem[];
  loading: boolean;
  error: string | null;
  selectedNamespaceId: string | null;
  selectedProjectPath: string | null;

  loadNamespaces: () => Promise<void>;
  createNamespace: (name: string, scope: string, embeddingProvider?: string) => Promise<MemoryNamespace | null>;
  deleteNamespace: (id: string) => Promise<void>;
  updateNamespace: (id: string, input: UpdateMemoryNamespaceInput) => Promise<void>;
  loadItems: (namespaceId: string) => Promise<void>;
  addItem: (namespaceId: string, title: string, content: string) => Promise<void>;
  deleteItem: (namespaceId: string, itemId: string) => Promise<void>;
  updateItem: (namespaceId: string, itemId: string, input: UpdateMemoryItemInput) => Promise<void>;
  setSelectedNamespaceId: (id: string | null) => void;
  reorderNamespaces: (namespaceIds: string[]) => Promise<void>;
  loadProjectProfiles: () => Promise<void>;
  getProjectProfile: (projectPath: string, projectName?: string | null) => Promise<ProjectMemoryProfile | null>;
  updateProjectProfile: (projectPath: string, projectName: string | null | undefined, input: UpdateMemoryNamespaceInput) => Promise<ProjectMemoryProfile | null>;
  loadProjectItems: (projectPath: string, projectName?: string | null) => Promise<void>;
  addProjectItem: (projectPath: string, projectName: string | null | undefined, title: string, content: string) => Promise<void>;
  setSelectedProjectPath: (projectPath: string | null) => void;
}

export const useMemoryStore = create<MemoryState>((set, get) => ({
  namespaces: [],
  projectProfiles: [],
  items: [],
  loading: false,
  error: null,
  selectedNamespaceId: null,
  selectedProjectPath: null,

  loadNamespaces: async () => {
    set({ loading: true });
    try {
      const namespaces = await invoke<MemoryNamespace[]>('list_memory_namespaces');
      set({ namespaces, loading: false, error: null });
    } catch (e) {
      set({ error: String(e), loading: false });
    }
  },

  createNamespace: async (name, scope, embeddingProvider) => {
    try {
      const ns = await invoke<MemoryNamespace>('create_memory_namespace', {
        input: { name, scope, embeddingProvider },
      });
      set((s) => ({ namespaces: [...s.namespaces, ns], error: null }));
      return ns;
    } catch (e) {
      set({ error: String(e) });
      return null;
    }
  },

  deleteNamespace: async (id) => {
    try {
      await invoke('delete_memory_namespace', { id });
      set((s) => ({ namespaces: s.namespaces.filter((n) => n.id !== id), error: null }));
    } catch (e) {
      set({ error: String(e) });
      throw e;
    }
  },

  updateNamespace: async (id, input) => {
    try {
      const updated = await invoke<MemoryNamespace>('update_memory_namespace', { id, input });
      set((s) => ({
        namespaces: s.namespaces.map((n) => (n.id === id ? updated : n)),
        error: null,
      }));
    } catch (e) {
      set({ error: String(e) });
      throw e;
    }
  },

  loadItems: async (namespaceId) => {
    set({ loading: true });
    try {
      const items = await invoke<MemoryItem[]>('list_memory_items', { namespaceId: namespaceId });
      set({ items, loading: false, error: null });
    } catch (e) {
      set({ error: String(e), loading: false });
    }
  },

  addItem: async (namespaceId, title, content) => {
    try {
      await invoke('add_memory_item', { input: { namespaceId, title, content } });
      await get().loadItems(namespaceId);
    } catch (e) {
      set({ error: String(e) });
      throw e;
    }
  },

  deleteItem: async (namespaceId, itemId) => {
    try {
      await invoke('delete_memory_item', { namespaceId, id: itemId });
      await get().loadItems(namespaceId);
    } catch (e) {
      set({ error: String(e) });
      throw e;
    }
  },

  updateItem: async (namespaceId, itemId, input) => {
    try {
      await invoke<MemoryItem>('update_memory_item', { namespaceId, id: itemId, input });
      await get().loadItems(namespaceId);
    } catch (e) {
      set({ error: String(e) });
      throw e;
    }
  },

  setSelectedNamespaceId: (id) => {
    set({ selectedNamespaceId: id });
  },

  reorderNamespaces: async (namespaceIds) => {
    await invoke('reorder_memory_namespaces', { namespaceIds });
    set((s) => {
      const ordered = namespaceIds
        .map((id, i) => {
          const n = s.namespaces.find((n) => n.id === id);
          return n ? { ...n, sortOrder: i } : null;
        })
        .filter(Boolean) as MemoryNamespace[];
      return { namespaces: ordered };
    });
  },

  loadProjectProfiles: async () => {
    set({ loading: true });
    try {
      const projectProfiles = await invoke<ProjectMemoryProfile[]>('list_project_memory_profiles');
      set((s) => ({
        projectProfiles,
        selectedProjectPath: s.selectedProjectPath ?? projectProfiles[0]?.projectPath ?? null,
        loading: false,
        error: null,
      }));
    } catch (e) {
      set({ error: String(e), loading: false });
    }
  },

  getProjectProfile: async (projectPath, projectName) => {
    try {
      const profile = await invoke<ProjectMemoryProfile>('get_project_memory_profile', {
        projectPath,
        projectName: projectName ?? null,
      });
      set((s) => {
        const exists = s.projectProfiles.some((p) => p.projectPath === profile.projectPath);
        return {
          projectProfiles: exists
            ? s.projectProfiles.map((p) => (p.projectPath === profile.projectPath ? profile : p))
            : [...s.projectProfiles, profile],
          error: null,
        };
      });
      return profile;
    } catch (e) {
      set({ error: String(e) });
      return null;
    }
  },

  updateProjectProfile: async (projectPath, projectName, input) => {
    try {
      const profile = await invoke<ProjectMemoryProfile>('update_project_memory_profile', {
        projectPath,
        projectName: projectName ?? null,
        input,
      });
      set((s) => ({
        projectProfiles: s.projectProfiles.map((p) => (p.projectPath === profile.projectPath ? profile : p)),
        error: null,
      }));
      return profile;
    } catch (e) {
      set({ error: String(e) });
      throw e;
    }
  },

  loadProjectItems: async (projectPath, projectName) => {
    set({ loading: true });
    try {
      const items = await invoke<MemoryItem[]>('list_project_memory_items', {
        projectPath,
        projectName: projectName ?? null,
      });
      set({ items, loading: false, error: null });
    } catch (e) {
      set({ error: String(e), loading: false });
    }
  },

  addProjectItem: async (projectPath, projectName, title, content) => {
    try {
      await invoke('add_project_memory_item', {
        projectPath,
        projectName: projectName ?? null,
        title,
        content,
      });
      await get().loadProjectItems(projectPath, projectName ?? null);
      await get().loadProjectProfiles();
    } catch (e) {
      set({ error: String(e) });
      throw e;
    }
  },

  setSelectedProjectPath: (projectPath) => {
    set({ selectedProjectPath: projectPath });
  },
}));
