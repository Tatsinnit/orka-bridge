import { invoke, isTauri } from '@tauri-apps/api/core';

export type WorkspaceRequirements = { memoryBytes: number; imageBytes: number; stateDiskBytes: number };
export async function workspaceRequirements(): Promise<WorkspaceRequirements | null> {
  if (!isTauri()) return null;
  try {
    const value = await invoke<unknown>('setup_requirements');
    if (typeof value !== 'object' || value === null || Array.isArray(value)) return null;
    if (Object.keys(value).sort().join(',') !== 'imageBytes,memoryBytes,stateDiskBytes') return null;
    const sizes = value as Record<string, unknown>;
    if (!Object.values(sizes).every(size => typeof size === 'number' && Number.isSafeInteger(size) && size > 0)) return null;
    return value as WorkspaceRequirements;
  } catch { return null; }
}

// Welcome requirements use decimal GB; transfer progress keeps its existing GiB units.
export function formatRequirementGB(bytes: number) {
  return `${new Intl.NumberFormat(undefined, { minimumFractionDigits: 1, maximumFractionDigits: 1 }).format(bytes / 1e9)} GB`;
}

export function formatSize(bytes: number) {
  return `${new Intl.NumberFormat(undefined, { maximumFractionDigits: 1 }).format(bytes / 1024 ** 3)} GiB`;
}
