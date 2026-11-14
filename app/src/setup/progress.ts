import type { ModelState } from './Model';
import { formatSize } from './requirements';
import type { SetupStatus } from './state';
import type { CheckStepState } from './Steps';
import type { Status } from '../components/DesignSystem';
import { preflightRecovery } from './preflight-state';
import { workspaceRecovery } from './recovery-info';

// One derivation feeds the tab, footer and step list so their counters cannot disagree.
// The stages are: install files, start, Cluster ready, connect model, first test.
export const setupSteps = 5;
// A frontend estimate only: native reports elapsed startup time, never a measured typical.
export const startupEstimateMs = 30_000;
// The bar keeps room for startup and readiness, so it cannot finish before native says ready.
const transferShare = 0.7;
const startupShare = 0.28;

// busy = the app is working (blue); needs-you = the creator's move (amber); ready = finished (green).
export type Tone = 'wait' | 'busy' | 'needs-you' | 'ready' | 'bad' | 'stale';
export type SetupView = { place: 'checking' | 'unavailable' | 'none' | 'local'; step: number; done: boolean; line: string; detail: string; tone: Tone; status: Status; action: string };

export const plural = (count: number, unit: string) => `${count} ${unit}${count === 1 ? '' : 's'}`;
// Native support must be affirmed: an unsupported platform or an older snapshot never offers the early choice.
export const canQueueModel = (status: SetupStatus | undefined) =>
  status?.modelSupported === true && status.selected === 'localVm' && ['preparing', 'installing', 'starting'].includes(status.localVm);
// Failures before the VM starts belong to stage 1; an unspecified failure matches the recovery default.
export function failedStep(status: SetupStatus): 1 | 2 {
  if (status.preflightFailure) return 1;
  const operation = status.workspaceFailure?.operation ?? 'start';
  return operation === 'start' || operation === 'services' || operation === 'stop' ? 2 : 1;
}

export function setupView(status: SetupStatus | undefined, { checking = false, stopping = false, model = 'idle', check, initialAutomatic = false }: { checking?: boolean; stopping?: boolean; model?: ModelState; check?: CheckStepState; initialAutomatic?: boolean } = {}): SetupView {
  const view = (place: SetupView['place'], step: number, line: string, detail: string, tone: Tone, done = false): SetupView => ({
    place, step, done, line, detail, tone,
    status: ({ busy: 'running', 'needs-you': 'needs-you', wait: 'not-started', ready: 'passed', bad: 'failed', stale: 'stale' } as const)[tone],
    action: done ? 'Manage' : step === 4 ? 'Connect the AI' : place === 'local' ? 'Continue setup' : 'Set up',
  });
  if (!status) return checking ? view('checking', 0, 'Checking the local cluster…', 'Checking the local cluster…', 'busy') : view('unavailable', 0, 'Local cluster unavailable', 'Local cluster unavailable', 'needs-you');
  const place = status.selected === 'localVm' ? 'local' : 'none';
  if (status.localVm === 'notIntegrated') return view(place, 0, 'Local cluster unavailable', 'Local cluster unavailable', 'needs-you');
  if (place === 'none') return view(place, 0, 'Choose where agents run', 'Choose where agents run', 'wait');
  if (stopping) return view(place, status.localVm === 'ready' ? 5 : 0, 'Stopping the local cluster', 'Stopping the local cluster', 'busy', status.localVm === 'ready');
  switch (status.localVm) {
    case 'starting': return view(place, 2, 'Starting the local cluster', 'Starting the local cluster', 'busy');
    case 'failed': {
      const line = status.preflightFailure ? preflightRecovery(status.preflightFailure).summary : 'Setup failed';
      return view(place, failedStep(status), line, status.preflightFailure ? line : workspaceRecovery(status.workspaceFailure).summary, 'bad');
    }
    case 'closing': return view(place, 0, 'Stopping the local cluster', 'Stopping the local cluster', 'busy');
    case 'closed': return view(place, 1, 'Local cluster stopped', 'Local cluster stopped', 'wait');
    case 'cancelled': return view(place, 1, 'Setup paused', 'Downloaded files are kept', 'wait');
    case 'idle': return view(place, 0, 'Not set up', 'Not set up', 'wait');
    case 'ready': {
      if (status.model === 'locked') return view(place, 4, 'AI unavailable', 'AI connection unavailable', 'wait');
      if (model === 'failed') return view(place, 4, 'AI needs attention', 'Check the AI connection in Clusters', 'bad');
      if (model !== 'connected') {
        const detail = { connecting: 'Connecting the AI…', removing: 'Removing the AI sign-in…', queued: 'AI queued', idle: 'Connect the AI' }[model];
        return view(place, 4, detail, detail, model === 'idle' ? 'needs-you' : model === 'queued' ? 'wait' : 'busy');
      }
      // Test evidence can need attention after setup; only the initial automatic run is a setup stage.
      const done = !initialAutomatic;
      const tone = check === 'failed' ? 'bad' : check === 'passed' ? 'ready' : check === 'running' ? 'busy' : check === 'stale' || check === 'unknown' ? 'stale' : initialAutomatic ? 'needs-you' : 'wait';
      const line = !done ? 'Run your first agent' : check ? { passed: 'Local cluster ready', failed: 'Its test failed', running: 'Running test', stale: 'Tests need running', unknown: 'Test result unknown' }[check] : 'Local cluster ready';
      return view(place, 5, line, 'AI connected', check || !done ? tone : 'ready', done);
    }
    default: return view(place, 1, 'Getting the local cluster ready', 'Getting the local cluster ready', 'busy');
  }
}

export type Infra = { value: number | null; text?: string; remaining?: string; estimate?: string };
// Null means no unfinished infrastructure stage; value null means the transfer size is not known yet.
export function infraProgress(status: SetupStatus): Infra | null {
  const progress = status.workspaceProgress;
  if (status.localVm === 'starting') {
    const elapsed = progress?.phase === 'starting' ? progress.elapsedMs : 0;
    const left = Math.ceil((startupEstimateMs - elapsed) / 1000);
    return {
      value: transferShare + startupShare * Math.min(elapsed / startupEstimateMs, 1),
      text: progress?.phase === 'starting' ? `${plural(Math.floor(elapsed / 1000), 'second')} elapsed` : 'Getting the local cluster ready…',
      estimate: left > 0 ? `Rough estimate, not a measurement: about ${plural(left, 'second')} left.` : 'Taking longer than the rough 30-second estimate. Still working.',
    };
  }
  if (status.localVm !== 'preparing' && status.localVm !== 'installing') return null;
  const bytes = progress?.phase === 'materializing' ? progress.bytes : null;
  const total = progress?.phase === 'materializing' ? progress.totalBytes : null;
  const remaining = progress?.phase === 'materializing' ? progress.remainingMs : null;
  if (bytes === null || total === null) return { value: null };
  return {
    value: transferShare * bytes / total, text: `Installing, ${formatSize(bytes)} of ${formatSize(total)}`,
    remaining: remaining !== null && remaining > 0 ? `${plural(Math.ceil(remaining / 1000), 'second')} left` : undefined,
  };
}
