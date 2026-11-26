import { expect, test } from 'vitest';
import { setupNeeds, setupTime, type WorkspaceRequirements } from './requirements';

const base: WorkspaceRequirements = { memoryBytes: 6442450944, stateDiskBytes: 34359738368, downloadBytes: 1967572914, setupDiskBytes: 9453699072, freeDiskBytes: 13250830695, readySeconds: null };

test('setup time names the download and adds a start time only where one was measured', () => {
  expect(setupTime(base)).toBe('Mostly a 2.0 GB download');
  expect(setupTime({ ...base, readySeconds: 40 })).toBe('Mostly a 2.0 GB download, then about 1 minute to start');
  expect(setupTime({ ...base, readySeconds: 10 })).toBe('Mostly a 2.0 GB download, then about 1 minute to start');
  expect(setupTime({ ...base, readySeconds: 134 })).toBe('Mostly a 2.0 GB download, then about 2 minutes to start');
  expect(setupTime({ ...base, downloadBytes: null, readySeconds: 134 })).toBe('Mostly a download');
  expect(setupTime(undefined)).toBe('Mostly a download');
});

test('disk need appears only when both the free and kept figures were measured', () => {
  expect(setupNeeds(base)).toBe('Downloads about 2.0 GB. Needs virtualization, 6.4 GB of memory and 13.3 GB of free disk; about 9.5 GB stays in use. Data disk can grow to 34.4 GB.');
  for (const missing of [{ freeDiskBytes: null }, { setupDiskBytes: null }]) {
    expect(setupNeeds({ ...base, ...missing })).toBe('Downloads about 2.0 GB. Needs virtualization and 6.4 GB of memory. Data disk can grow to 34.4 GB.');
  }
  // 24.21 GB would round down to 24.2; the need rounds up instead.
  expect(setupNeeds({ ...base, freeDiskBytes: 24210000000 })).toContain('24.3 GB of free disk');
  expect(setupNeeds({ ...base, freeDiskBytes: 24200000000 })).toContain('24.2 GB of free disk');
});
