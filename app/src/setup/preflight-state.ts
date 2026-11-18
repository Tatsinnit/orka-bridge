import { invoke, isTauri } from '@tauri-apps/api/core';
import type { RecoveryInfo } from './recovery-info';

export const failures = ['virtualizationDisabled', 'hypervisorUnavailable', 'virtualizationUnsupported', 'insufficientMemory'] as const;
export type PreflightFailure = (typeof failures)[number];
export type HostPreflight = {
  platform: 'windows' | 'macos'; virtualization: 'ready' | 'disabled' | 'hypervisorUnavailable' | 'unsupported' | 'unknown';
  memory: 'ready' | 'insufficient' | 'unknown'; installedMemoryBytes: number | null; requiredMemoryBytes: number;
};
export const isPreflightFailure = (value: unknown): value is PreflightFailure => failures.some(f => f === value);
export async function readPreflight(required: number): Promise<HostPreflight | null> {
  if (!isTauri()) return null;
  try {
    const value: unknown = await invoke('setup_preflight');
    if (typeof value !== 'object' || value === null || Array.isArray(value)
      || Object.keys(value).sort().join(',') !== 'installedMemoryBytes,memory,platform,requiredMemoryBytes,virtualization') return null;
    const p = value as Record<string, unknown>;
    if (!['windows', 'macos'].includes(p.platform as string)
      || !['ready', 'disabled', 'hypervisorUnavailable', 'unsupported', 'unknown'].includes(p.virtualization as string)
      || !['ready', 'insufficient', 'unknown'].includes(p.memory as string)
      || p.requiredMemoryBytes !== required
      || (p.installedMemoryBytes !== null && !(typeof p.installedMemoryBytes === 'number' && Number.isSafeInteger(p.installedMemoryBytes) && p.installedMemoryBytes > 0))
      || ((p.memory === 'unknown') !== (p.installedMemoryBytes === null))
      || (p.platform === 'macos' && (p.virtualization === 'disabled' || p.virtualization === 'hypervisorUnavailable'))
      || (p.platform === 'windows' && p.virtualization === 'unsupported')) return null;
    return value as HostPreflight;
  } catch { return null; }
}
export function preflightRecovery(failure: PreflightFailure): RecoveryInfo {
  const stage = 'Checking computer requirements';
  switch (failure) {
    case 'insufficientMemory': return { summary: 'This computer has less installed memory than the workspace needs.', action: 'Use a computer with more installed memory, or use a cluster instead.', stage };
    case 'virtualizationUnsupported': return { summary: "Virtualization isn't supported on this Mac.", action: 'Use a supported Apple-silicon Mac, or use a cluster instead.', stage };
    case 'virtualizationDisabled': return { summary: 'Virtualization is turned off or unavailable on this PC.', action: 'Check whether this PC supports virtualization and enable it in its startup settings, then restart and try again.', topic: 'virtualization', stage };
    case 'hypervisorUnavailable': return { summary: "Windows Hypervisor Platform isn't available.", action: 'Enable Windows Hypervisor Platform in Windows features, restart, then try again.', topic: 'hypervisor', stage };
  }
}
