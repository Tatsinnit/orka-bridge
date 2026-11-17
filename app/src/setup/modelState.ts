import { invoke, isTauri } from '@tauri-apps/api/core';
import { isOsCode } from './recovery-info';

const errors = {
  modelCaptureUnavailable: 'Install GitHub CLI and sign in to github.com before connecting.',
  modelCaptureFailed: 'The stored GitHub sign-in could not be read. Check your GitHub CLI sign-in.',
  modelCaptureInvalid: 'The stored GitHub sign-in returned invalid credential data.',
  modelCaptureTimedOut: 'Reading the stored GitHub sign-in timed out.',
  modelStore: 'Bridge could not save or load your sign-in.',
  modelGuest: 'The workspace is not ready to connect the AI.',
  modelDelivery: 'Bridge could not confirm that the workspace received the sign-in.',
  modelConnect: 'Bridge could not confirm the AI connection. Check again to reconnect.',
  modelBusy: 'Another operation is using the workspace. Wait for it to finish.',
  modelSession: 'The workspace is not available to connect the AI.',
  modelUnavailable: 'The AI connection is unavailable or could not be confirmed.',
  modelInstallationMismatch: "Bridge's saved AI connection is from an earlier setup.",
  modelReconnect: 'Bridge could not clean up the previous model connection. The workspace may need a newer image. The saved sign-in was kept.',
  modelKeyInvalid: 'Paste the whole key.',
  modelAlreadyAssociated: 'Change or remove the current AI connection before adding a key.',
  modelCredentialRejected: 'GitHub Copilot did not accept the saved sign-in. Check your GitHub CLI sign-in.',
  modelProviderUnreachable: 'The workspace could not reach GitHub Copilot. Check your internet connection.',
  modelModelUnavailable: 'The default model is not listed for this Copilot sign-in. Check your Copilot plan.',
  modelKeyCleanup: 'Key removal has not been confirmed. Retry removal before adding another key.',
};
export type KeyVendor = 'openai' | 'anthropic';
export const companies = {
  openai: { name: 'OpenAI', model: 'gpt-4.1', prefix: 'sk-', site: 'platform.openai.com' },
  anthropic: { name: 'Anthropic', model: 'claude-sonnet-5-5', prefix: 'sk-ant-', site: 'console.anthropic.com' },
} as const;
export const copilotModel = 'gpt-5.6-terra';
// `account` is the GitHub login verified when this sign-in was saved; absent means unavailable.
export type ModelConnection = { kind: 'copilot'; model: typeof copilotModel; account?: string } | { kind: 'key'; vendor: KeyVendor; model: string };
const githubLogin = /^[A-Za-z0-9][A-Za-z0-9-]{0,38}$/;
function connectionInfo(value: unknown): ModelConnection | undefined {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return;
  const c = value as Record<string, unknown>;
  const keys = Object.keys(c).sort().join(',');
  if (c.kind === 'copilot' && c.model === copilotModel) {
    if (keys === 'kind,model') return { kind: 'copilot', model: c.model };
    if (keys === 'account,kind,model' && typeof c.account === 'string' && githubLogin.test(c.account)) return { kind: 'copilot', model: c.model, account: c.account };
    return;
  }
  if (Object.keys(c).sort().join(',') === 'kind,model,vendor' && c.kind === 'key' && (c.vendor === 'openai' || c.vendor === 'anthropic') && c.model === companies[c.vendor].model) return { kind: 'key', vendor: c.vendor, model: companies[c.vendor].model };
}
export function validKey(vendor: KeyVendor, key: string) {
  return key.length > companies[vendor].prefix.length && key.length <= 16384 && /^[\x21-\x7e]+$/.test(key)
    && key.startsWith(companies[vendor].prefix) && (vendor !== 'openai' || !key.startsWith('sk-ant-'));
}
export function keyFailure(error: ModelFailure, vendor: KeyVendor): string {
  const c = companies[vendor];
  switch (error.category) {
    case 'modelKeyInvalid': return `Paste the whole key. ${c.name} keys start with ${c.prefix}.`;
    case 'modelCredentialRejected': return `${c.name} didn't accept this key. Copy it again from ${c.site}, or create a new one.`;
    case 'modelProviderUnreachable': return `Your workspace couldn't reach ${c.name}. Check this computer's internet connection, then try again.`;
    case 'modelModelUnavailable': return `${c.model} isn't listed for this key. Check your ${c.name} plan, or choose Copilot.`;
    default: return error.message.replaceAll('sign-in', 'key');
  }
}
export type ModelRecovery = { installation: string; savedInstallation: string; credentialSlot: string; resetStage?: 'credentialRemovalUnconfirmed' | 'recordRemovalUnconfirmed' };
function recoveryInfo(value: unknown): ModelRecovery | undefined {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return;
  const r = value as Record<string, unknown>;
  const uuid = (id: unknown): id is string => typeof id === 'string' && /^[a-f0-9]{8}(?:-[a-f0-9]{4}){3}-[a-f0-9]{12}$/i.test(id) && id !== '00000000-0000-0000-0000-000000000000';
  if (!uuid(r.installation) || !uuid(r.savedInstallation) || !uuid(r.credentialSlot) || r.installation.toLowerCase() === r.savedInstallation.toLowerCase()) return;
  if (Object.keys(r).some(key => !['installation', 'savedInstallation', 'credentialSlot', 'resetStage'].includes(key))) return;
  if (r.resetStage !== undefined && r.resetStage !== 'credentialRemovalUnconfirmed' && r.resetStage !== 'recordRemovalUnconfirmed') return;
  return { installation: r.installation, savedInstallation: r.savedInstallation, credentialSlot: r.credentialSlot, ...(r.resetStage ? { resetStage: r.resetStage } : {}) };
}
const cleanupOperations = ['modelStatus', 'modelStatusPreconditions', 'disconnectRetainedModel'] as const;
const cleanupClasses = ['timeout', 'unsupported', 'protocol', 'connection', 'permission', 'refused', 'io'] as const;
type CleanupDiagnostic = [typeof cleanupOperations[number], typeof cleanupClasses[number]];
function cleanupInfo(value: unknown): CleanupDiagnostic | undefined {
  if (!Array.isArray(value) || value.length !== 2) return;
  const operation = cleanupOperations.find(item => item === value[0]);
  const classification = cleanupClasses.find(item => item === value[1]);
  return operation && classification ? [operation, classification] : undefined;
}
export type ModelFailure = { message: string; category: keyof typeof errors | 'unknown'; codes: number[]; vendor?: KeyVendor; recovery?: ModelRecovery; cleanup?: CleanupDiagnostic };
export type ModelResult = { ok: true; connection?: ModelConnection } | ({ ok: false } & ModelFailure);
export const connectModel = (vendor?: KeyVendor) => call('setup_connect_model', true, 'The AI connection could not be completed.', vendor ? { vendor } : undefined);
export const resetModel = () => call('setup_reset_model', false, 'Bridge could not confirm the AI connection was reset.');
export const removeModel = () => call('setup_remove_model', false, 'Bridge could not confirm sign-in removal. Try Remove sign-in again.');
export const connectKey = (vendor: KeyVendor, key: string) => call('setup_connect_key', true, 'The AI connection could not be completed.', { vendor, key });
async function call(command: string, ready: boolean, fallback: string, args?: { vendor: KeyVendor; key?: string }): Promise<ModelResult> {
  if (!isTauri()) return { ok: false, category: 'unknown', codes: [], message: 'AI setup is not available in this preview.' };
  try {
    const value = await (args ? invoke<unknown>(command, args) : invoke<unknown>(command));
    if (typeof value === 'object' && value !== null && !Array.isArray(value) && 'ready' in value && value.ready === ready) {
      const keys = Object.keys(value).sort().join(',');
      if (keys === 'ready' && !args) return { ok: true };
      if (ready && keys === 'connection,ready' && 'connection' in value) {
        const connection = connectionInfo(value.connection);
        if (connection && (!args || connection.kind === 'key' && connection.vendor === args.vendor)) return { ok: true, connection };
      }
    }
  } catch (error: unknown) {
    // Only a native finite category selects text. Never render a supplied message.
    if (typeof error === 'object' && error !== null && 'kind' in error
      && typeof error.kind === 'string' && Object.hasOwn(errors, error.kind)) {
      const category = error.kind as keyof typeof errors;
      const vendor = 'vendor' in error && (error.vendor === 'openai' || error.vendor === 'anthropic') ? error.vendor : undefined;
      const recovery = 'recovery' in error ? recoveryInfo(error.recovery) : undefined;
      const cleanup = 'cleanup' in error ? cleanupInfo(error.cleanup) : undefined;
      const context: Pick<ModelFailure, 'vendor' | 'recovery' | 'cleanup'> = { ...(vendor ? { vendor } : {}), ...(recovery ? { recovery } : {}), ...(cleanup ? { cleanup } : {}) };
      if (category === 'modelStore' && 'native' in error) {
        const detail = error.native;
        if (typeof detail === 'object' && detail !== null && 'call' in detail && 'code' in detail
          && (detail.call === 'CredWriteW' || detail.call === 'CredReadW' || detail.call === 'CredDeleteW')
          && isOsCode(detail.code)) {
          const operation = detail.call === 'CredWriteW' ? 'save' : detail.call === 'CredReadW' ? 'load' : 'remove';
          const reason = detail.call === 'CredWriteW' && detail.code === 8
            ? "Windows couldn't save the sign-in. Windows Credential Manager is full"
            : `Windows couldn't ${operation} the sign-in`;
          return { ok: false, category, codes: [detail.code], message: `${reason} (${detail.call}, error ${detail.code}).`, ...context };
        }
      }
      return { ok: false, category, codes: [], message: errors[category], ...context };
    }
  }
  return { ok: false, category: 'unknown', codes: [], message: fallback };
}
