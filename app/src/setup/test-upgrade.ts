import type { UpgradeStatus } from './upgrade-api';

// Ordinary workspace fixtures explicitly model the native no-upgrade observation.
export const noUpgrade: UpgradeStatus = {
  state: 'none', transaction: null, createdAt: null, sourceReleases: [], targetRelease: null,
  retainedAllocatedBytes: null, additionalBytes: null, credential: 'absent', busy: false,
  error: null, repeated: false, canReplace: false, canRestore: false, canCancel: false,
};
