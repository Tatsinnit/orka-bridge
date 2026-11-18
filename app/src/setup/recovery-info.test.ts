import { expect, test } from 'vitest';
import { helpDetails, isWorkspaceFailure, workspaceRecovery, type WorkspaceFailure } from './recovery-info';

const failure = (cause: WorkspaceFailure['cause'], operation: WorkspaceFailure['operation'] = 'start', codes: number[] = []): WorkspaceFailure => ({ cause, operation, codes });

test('workspace metadata only accepts finite categories and at most four unsigned native codes', () => {
  expect(isWorkspaceFailure(failure('permission', 'start', [0, 0xffffffff]))).toBe(true);
  for (const value of [null, [], {}, failure('unknown', 'start', [-1]), failure('unknown', 'start', [1.5]),
    failure('unknown', 'start', [0x100000000]), failure('unknown', 'start', [1, 2, 3, 4, 5]),
    { ...failure('unknown'), operation: 'PRIVATE' }, { ...failure('unknown'), cause: 'PRIVATE' },
    { ...failure('unknown'), secret: 'PRIVATE' }]) expect(isWorkspaceFailure(value)).toBe(false);
});

test('cause and operation select plain copy without interpreting diagnostics', () => {
  const denied = workspaceRecovery(failure('permission'));
  expect(denied.summary).toContain('does not have permission');
  expect(denied.heading).toBe("The workspace couldn't start");
  const download = workspaceRecovery(failure('network', 'download'));
  expect(download.heading).toBe("The workspace couldn't be downloaded");
  expect(download.preservation).toContain('Your agent files are kept');
  expect(download.preservation).not.toContain('What you downloaded is kept');
  expect(workspaceRecovery(failure('preparation', 'prepare')).preservation).toContain('existing cluster and agent files are kept');
  expect(workspaceRecovery(failure('stopUnconfirmed', 'stop')).retryBlocked).toBe(true);
  expect(workspaceRecovery().summary).toContain("Your workspace didn't start");
});

test.each(['workspaceUpdateRequired', 'workspaceVersionUnsupported'] as const)('binding refusal %s stays distinct from a corrupt download', cause => {
  const metadata = { cause, operation: 'verify' as const, codes: [] };
  expect(isWorkspaceFailure(metadata)).toBe(true);
  const info = workspaceRecovery(metadata);
  expect(info.summary).not.toContain("couldn't be verified");
  expect(info.action).toBe('Open the matching Bridge version.');
  expect(info.retryBlocked).toBe(true);
  expect(info.preservation).toContain('existing cluster and agent files are kept');
  expect(helpDetails(info)).toContain(`Category: ${cause}`);
});

test('confirmed forced stop is not described as clean or still running', () => {
  const metadata = { cause: 'forcedStop', operation: 'stop', codes: [] };
  expect(isWorkspaceFailure(metadata)).toBe(true);
  const info = workspaceRecovery(metadata as WorkspaceFailure);
  expect(info.retryBlocked).toBe(true);
  expect(info.summary).toContain('forcibly stopped');
  expect(info.action).toContain('reopen');
  expect(info.preservation).toContain('may be lost');
  expect(info.action).not.toContain('may still be stopping');
});

test('copied help is fixed labels and accepted codes, not a free-form summary or diagnostic', () => {
  const info = workspaceRecovery(failure('permission', 'start', [5]));
  const copied = helpDetails(info, [5]);
  expect(copied).toContain('Operation: Starting the workspace');
  expect(copied).toContain('Category: permission');
  expect(copied).toContain('OS code: 5');
  expect(copied).not.toContain(info.summary);
});
