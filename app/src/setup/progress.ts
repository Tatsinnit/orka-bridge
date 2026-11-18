import type { Status } from '../components/DesignSystem';
import type { ModelState } from './Model';
import { preflightRecovery } from './preflight-state';
import { workspaceRecovery } from './recovery-info';
import { formatRequirementGB } from './requirements';
import type { CheckStepState } from './Steps';
import type { SetupStatus } from './state';

// One derivation feeds the tab, footer and step list so their counters cannot disagree.
// Infrastructure substages share one workspace step; the file bar measures only file work.
export const setupSteps = 4;

// busy = the app is working (blue); needs-you = the creator's move (amber); ready = finished (green).
export type Tone = 'wait' | 'busy' | 'needs-you' | 'ready' | 'bad' | 'stale';
export type SetupView = { place: 'checking' | 'unavailable' | 'none' | 'local'; step: number; done: boolean; line: string; tabLine: string; footerLine: string; detail: string; tone: Tone; status: Status; action: string };

export const plural = (count: number, unit: string) => `${count} ${unit}${count === 1 ? '' : 's'}`;
// Native support must be affirmed: an unsupported platform or an older snapshot never offers the early choice.
export const canQueueModel = (status: SetupStatus | undefined) =>
  status?.modelSupported === true && status.selected === 'localVm' && ['preparing', 'installing', 'starting'].includes(status.localVm);
export function workspaceView(status: SetupStatus, { stopping = false, starting = false }: { stopping?: boolean; starting?: boolean } = {}): { word: string; detail: string; status: Status; tone: Tone } {
  if (stopping || status.localVm === 'closing') return { word: 'Stopping', detail: 'Stopping the workspace. Your agents and files are kept.', status: 'running', tone: 'busy' };
  if (starting || status.selected === 'localVm' && status.localVm === 'idle' || ['preparing', 'installing', 'starting'].includes(status.localVm)) return { word: 'Starting', detail: 'Starting the workspace.', status: 'running', tone: 'busy' };
  if (status.localVm === 'failed') return { word: status.workspaceFailure?.operation === 'stop' ? "Couldn't stop" : "Couldn't start", detail: status.workspaceFailure?.operation === 'stop' ? 'The workspace may still be stopping. Quit Bridge and reopen it before trying again.' : "The workspace couldn't start. Your agents and files are kept.", status: 'failed', tone: 'bad' };
  if (status.localVm === 'ready') return { word: 'Ready', detail: 'The workspace is ready on this computer.', status: 'passed', tone: 'ready' };
  return { word: 'Stopped', detail: 'The workspace is stopped. Your agents and their files are kept.', status: 'not-started', tone: 'wait' };
}
export function setupView(status: SetupStatus | undefined, { checking = false, stopping = false, starting = false, workspace = false, model = 'idle', check, initialAutomatic = false }: { checking?: boolean; stopping?: boolean; starting?: boolean; workspace?: boolean; model?: ModelState; check?: CheckStepState; initialAutomatic?: boolean } = {}): SetupView {
  const view = (place: SetupView['place'], step: number, line: string, detail: string, tone: Tone, done = false): SetupView => ({
    place, step, done, line, detail, tone,
    tabLine: !done && step > 0 && (tone === 'busy' || tone === 'needs-you') ? `Setting up, step ${step} of ${setupSteps}` : line,
    footerLine: !done && step > 0 && (tone === 'busy' || tone === 'needs-you') ? `${step === 2 ? 'Getting your workspace ready' : line}, step ${step} of ${setupSteps}` : line,
    status: ({ busy: 'running', 'needs-you': 'needs-you', wait: 'not-started', ready: 'passed', bad: 'failed', stale: 'stale' } as const)[tone],
    action: done ? 'Manage' : step === 3 ? 'Connect the AI' : place === 'local' ? 'Continue setup' : 'Set up',
  });
  if (!status) return checking ? view('checking', 0, 'Checking the workspace…', 'Checking the workspace…', 'busy') : view('unavailable', 0, 'Workspace unavailable', 'Workspace unavailable', 'needs-you');
  if (workspace) {
    const state = workspaceView(status, { stopping, starting });
    const result = view('local', 0, `Workspace ${state.word.toLowerCase()}`, state.detail, state.tone, status.localVm === 'ready');
    return { ...result, tabLine: `This computer, ${state.word.toLowerCase()}`, action: state.word === 'Stopped' && status.localVm === 'cancelled' ? 'Start' : 'Manage' };
  }
  const place = status.selected === 'localVm' ? 'local' : 'none';
  if (status.localVm === 'notIntegrated') return view(place, 0, 'Workspace unavailable', 'Workspace unavailable', 'needs-you');
  if (place === 'none') return view(place, 0, 'Choose where agents run', 'Choose where agents run', 'wait');
  if (stopping) return view(place, status.localVm === 'ready' ? 4 : 0, 'Stopping the workspace', 'Stopping the workspace', 'busy', status.localVm === 'ready');
  switch (status.localVm) {
    case 'starting': return view(place, 2, 'Starting the workspace', 'Starting the workspace', 'busy');
    case 'failed': {
      const line = status.preflightFailure ? preflightRecovery(status.preflightFailure).summary : 'Setup failed';
      return view(place, 2, line, status.preflightFailure ? line : workspaceRecovery(status.workspaceFailure).summary, 'bad');
    }
    case 'closing': return view(place, 0, 'Stopping the workspace', 'Stopping the workspace', 'busy');
    case 'closed': return view(place, 2, 'Workspace stopped', 'Workspace stopped', 'wait');
    case 'cancelled': return view(place, 2, 'Setup paused', 'Downloaded files are kept', 'wait');
    case 'idle': return view(place, 0, 'Not set up', 'Not set up', 'wait');
    case 'ready': {
      if (status.model === 'locked') return view(place, 3, 'AI not connected', 'AI not connected', 'wait');
      if (model === 'failed' || model === 'resetFailed' || model === 'removalFailed') return view(place, 3, 'AI not connected', 'The AI needs to be set up again', 'needs-you');
      if (model !== 'connected') {
        const detail = { resetting: 'Resetting the AI connection…', connecting: 'Connecting the AI…', removing: 'Removing the AI sign-in…', queued: 'AI queued', idle: 'Connect the AI' }[model];
        return view(place, 3, detail, detail, model === 'idle' ? 'needs-you' : model === 'queued' ? 'wait' : 'busy');
      }
      // Test evidence can need attention after setup; only the initial automatic run is a setup stage.
      const done = !initialAutomatic;
      const tone = check === 'failed' ? 'bad' : check === 'passed' ? 'ready' : check === 'running' ? 'busy' : check === 'stale' || check === 'unknown' ? 'stale' : initialAutomatic ? 'needs-you' : 'wait';
      const line = !done ? 'Run your first agent' : check ? { passed: 'Workspace ready', failed: 'Its test failed', running: 'Running test', stale: 'Tests need running', unknown: 'Test result unknown' }[check] : 'Workspace ready';
      return view(place, 4, line, 'AI connected', check || !done ? tone : 'ready', done);
    }
    default: return view(place, 2, 'Getting the workspace ready', 'Getting the workspace ready', 'busy');
  }
}

export type Infra = { value: number | null; text?: string; remaining?: string };
// Null means no unfinished infrastructure stage; value null means the transfer size is not known yet.
export function infraProgress(status: SetupStatus): Infra | null {
  const progress = status.workspaceProgress;
  if (status.localVm === 'starting') {
    return progress?.phase === 'starting'
      ? { value: null, text: `${plural(Math.floor(progress.elapsedMs / 1000), 'second')} elapsed` }
      : { value: null };
  }
  if (status.localVm !== 'preparing' && status.localVm !== 'installing') return null;
  const bytes = progress?.phase === 'materializing' ? progress.bytes : null;
  const total = progress?.phase === 'materializing' ? progress.totalBytes : null;
  const remaining = progress?.phase === 'materializing' ? progress.remainingMs : null;
  if (bytes === null || total === null) return { value: null };
  return {
    value: bytes / total, text: `Downloading, ${formatRequirementGB(bytes).replace(' GB', '')} of ${formatRequirementGB(total)}`,
    remaining: remaining !== null && remaining > 0 ? `${plural(Math.ceil(remaining / 1000), 'second')} left` : undefined,
  };
}
