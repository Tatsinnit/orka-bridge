import { invoke, isTauri } from '@tauri-apps/api/core';

// Disk and time come from measured first runs of the pinned image; null where not measured.
export type WorkspaceRequirements = {
  memoryBytes: number; stateDiskBytes: number; downloadBytes: number | null;
  setupDiskBytes: number | null; freeDiskBytes: number | null; readySeconds: number | null;
};
const measured = ['downloadBytes', 'freeDiskBytes', 'readySeconds', 'setupDiskBytes'];
export async function workspaceRequirements(): Promise<WorkspaceRequirements | null> {
  if (!isTauri()) return null;
  try {
    const value = await invoke<unknown>('setup_requirements');
    if (typeof value !== 'object' || value === null || Array.isArray(value)) return null;
    if (Object.keys(value).sort().join(',') !== 'downloadBytes,freeDiskBytes,memoryBytes,readySeconds,setupDiskBytes,stateDiskBytes') return null;
    const positive = (size: unknown) => typeof size === 'number' && Number.isSafeInteger(size) && size > 0;
    if (!Object.entries(value).every(([key, size]) => positive(size) || measured.includes(key) && size === null)) return null;
    return value as WorkspaceRequirements;
  } catch { return null; }
}

// Welcome requirements use decimal GB; transfer progress keeps its existing GiB units.
export function formatRequirementGB(bytes: number) {
  return `${new Intl.NumberFormat(undefined, { minimumFractionDigits: 1, maximumFractionDigits: 1 }).format(bytes / 1e9)} GB`;
}

// A needed amount rounds up, so meeting the shown figure always meets the check.
function formatNeededGB(bytes: number) {
  return formatRequirementGB(Math.ceil(bytes / 1e8) * 1e8);
}

export function setupTime(requirements: WorkspaceRequirements | null | undefined) {
  if (!requirements || requirements.downloadBytes === null) return 'Mostly a download';
  const download = `Mostly a ${formatRequirementGB(requirements.downloadBytes)} download`;
  if (requirements.readySeconds === null) return download;
  const minutes = Math.max(1, Math.round(requirements.readySeconds / 60));
  return `${download}, then about ${minutes} minute${minutes === 1 ? '' : 's'} to start`;
}

export function setupNeeds(requirements: WorkspaceRequirements) {
  const download = requirements.downloadBytes === null ? '' : `Downloads about ${formatRequirementGB(requirements.downloadBytes)}. `;
  const memory = `${formatRequirementGB(requirements.memoryBytes)} of memory`;
  const needs = requirements.freeDiskBytes === null || requirements.setupDiskBytes === null
    ? `Needs virtualization and ${memory}.`
    : `Needs virtualization, ${memory} and ${formatNeededGB(requirements.freeDiskBytes)} of free disk; about ${formatRequirementGB(requirements.setupDiskBytes)} stays in use.`;
  return `${download}${needs} Data disk can grow to ${formatRequirementGB(requirements.stateDiskBytes)}.`;
}

export function formatSize(bytes: number) {
  return `${new Intl.NumberFormat(undefined, { maximumFractionDigits: 1 }).format(bytes / 1024 ** 3)} GiB`;
}
