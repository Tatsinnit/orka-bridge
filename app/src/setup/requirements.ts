import { invoke, isTauri } from '@tauri-apps/api/core';

export type WorkspaceRequirements = { memoryBytes: number; imageBytes: number; stateDiskBytes: number; downloadBytes: number | null };
export async function workspaceRequirements(): Promise<WorkspaceRequirements | null> {
  if (!isTauri()) return null;
  try {
    const value = await invoke<unknown>('setup_requirements');
    if (typeof value !== 'object' || value === null || Array.isArray(value)) return null;
    if (Object.keys(value).sort().join(',') !== 'downloadBytes,imageBytes,memoryBytes,stateDiskBytes') return null;
    const { downloadBytes, ...sizes } = value as Record<string, unknown>;
    const positiveBytes = (size: unknown) => typeof size === 'number' && Number.isSafeInteger(size) && size > 0;
    if (!Object.values(sizes).every(positiveBytes) || downloadBytes !== null && !positiveBytes(downloadBytes)) return null;
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
