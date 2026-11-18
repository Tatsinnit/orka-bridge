export type HelpTopic = 'virtualization' | 'hypervisor' | 'workspace' | 'githubCliInstall' | 'githubSignIn' | 'openaiKey' | 'anthropicKey';
export type RecoveryInfo = { summary: string; action: string; topic?: HelpTopic; stage: string; retryBlocked?: boolean; heading?: string; preservation?: string; category?: string };
const operations = ['requirements', 'download', 'verify', 'prepare', 'start', 'services', 'stop'] as const;
const causes = ['permission', 'noSpace', 'integrity', 'workspaceUpdateRequired', 'workspaceVersionUnsupported', 'inUse', 'network', 'preparation', 'guestConnection', 'services', 'stopUnconfirmed', 'forcedStop', 'unknown'] as const;
export type WorkspaceFailure = { operation: typeof operations[number]; cause: typeof causes[number]; codes: number[] };
export const isOsCode = (value: unknown): value is number => typeof value === 'number' && Number.isInteger(value) && value >= 0 && value <= 0xffffffff;
export function isWorkspaceFailure(value: unknown): value is WorkspaceFailure {
  if (!value || typeof value !== 'object' || Array.isArray(value) || Object.keys(value).sort().join(',') !== 'cause,codes,operation') return false;
  const item = value as Record<string, unknown>;
  return operations.some(operation => operation === item.operation) && causes.some(cause => cause === item.cause)
    && Array.isArray(item.codes) && item.codes.length <= 4 && item.codes.every(isOsCode);
}
const operationLabels: Record<WorkspaceFailure['operation'], string> = {
  requirements: 'Checking requirements', download: 'Downloading the workspace', verify: 'Checking cluster files',
  prepare: 'Preparing the workspace', start: 'Starting the workspace', services: 'Starting cluster services', stop: 'Stopping the workspace',
};
// Only native finite metadata selects a cause. Private diagnostics are never input.
export function workspaceRecovery(failure?: WorkspaceFailure): RecoveryInfo {
  const operation = failure?.operation ?? 'start';
  const cause = failure?.cause ?? 'unknown';
  const common = { stage: operationLabels[operation], category: cause,
    heading: operation === 'download' ? "The workspace couldn't be downloaded" : operation === 'prepare' || operation === 'verify' ? "The workspace couldn't be prepared" : "The workspace couldn't start",
    preservation: operation === 'requirements' ? 'This check made no changes.'
      : operation === 'download' ? 'Your agent files are kept. Try again to continue setup.'
      : operation === 'prepare' || operation === 'verify' ? 'Your existing cluster and agent files are kept.'
      : 'Your workspace and agent files are kept.',
  };
  switch (cause) {
    case 'permission': return { ...common, summary: operation === 'prepare' || operation === 'verify' ? 'Bridge does not have permission to prepare the workspace.' : 'Bridge does not have permission to start the workspace.', action: 'Check Bridge file permissions, then press Try again.', topic: 'workspace' };
    case 'noSpace': return { ...common, summary: "There isn't enough free space for the workspace.", action: 'Free some space on this computer, then press Try again.', topic: 'workspace' };
    case 'integrity': return { ...common, summary: "The cluster download couldn't be verified.", action: 'Check that the cluster files match this Bridge build, then press Try again.', topic: 'workspace' };
    case 'workspaceUpdateRequired': return { ...common, summary: 'This version of Bridge needs a new workspace.', action: 'Open the matching Bridge version.', retryBlocked: true };
    case 'workspaceVersionUnsupported': return { ...common, summary: 'This workspace uses different cluster files.', action: 'Open the matching Bridge version.', retryBlocked: true };
    case 'inUse': return { ...common, summary: 'The cluster files are still in use or need checking.', action: 'Close any other Bridge window, then press Try again.', topic: 'workspace' };
    case 'network': return { ...common, summary: "The workspace couldn't be downloaded.", action: 'Check your internet connection, then press Try again.' };
    case 'preparation': return { ...common, summary: "The workspace couldn't be prepared.", action: 'Press Try again. If it happens again, copy the details for help.' };
    case 'guestConnection': return { ...common, summary: "The workspace didn't finish starting.", action: 'Press Try again to restart it. If it happens again, copy the details for help.' };
    case 'services': return { ...common, summary: "The workspace's services couldn't start.", action: 'Press Try again to restart the workspace. If it happens again, copy the details for help.' };
    case 'forcedStop': return { ...common, heading: 'The workspace was forcibly stopped', summary: 'The workspace was forcibly stopped without a clean shutdown.', action: 'Quit Bridge and reopen it before trying again.', preservation: 'Changes that had not reached disk may be lost.', topic: 'workspace', retryBlocked: true };
    case 'stopUnconfirmed': return { ...common, summary: "The workspace couldn't be stopped safely.", action: 'Quit Bridge and reopen it before trying again. The workspace may still be stopping.', topic: 'workspace', retryBlocked: true };
    default: return { ...common, summary: "Your workspace didn't start. Your agents are safe.", action: '', preservation: '' };
  }
}

export function helpDetails(info: RecoveryInfo, codes: number[] = []): string {
  return ['Bridge workspace setup', `Operation: ${info.stage}`, `Category: ${info.category ?? 'requirements'}`,
    ...[...new Set(codes.filter(isOsCode))].slice(0, 4).map(code => `OS code: ${code}`), 'Private diagnostic text is not included.'].join('\n');
}

export function modelDetails(detail: string): string {
  const marker = detail.indexOf(' Guest Connect:');
  return marker >= 0 ? detail.slice(marker + 1) : detail;
}
