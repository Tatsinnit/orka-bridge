import { Disclosure } from '../components/Disclosure';
import { Button } from '../components/Button';
import { type ReactNode, useEffect, useState } from 'react';
import { agentInfo } from '../designer/agent-api';
import { type ExamplePair, loadExample, type SetupStatus } from './state';
import Recovery from './Recovery';
import { StatusIcon, StatusPill, type Status } from '../components/DesignSystem';
import type { ModelState } from './Model';
import { canQueueModel, infraProgress, setupView } from './progress';
import { modelDetails } from './recovery-info';
import './steps.css';

export type CheckStepState = 'running' | 'passed' | 'failed' | 'unknown' | 'stale';

type Props = {
  status: SetupStatus; busy: boolean; stopping: boolean; error?: string; modelState?: ModelState; firstCheck?: CheckStepState;
  firstTestResult?: { failed: number }; initialAutomatic?: boolean; onCheckFirstTest?: () => void;
  onCancel: () => void; onRestart: () => void; onCluster?: () => void; onChange: (button: HTMLButtonElement) => void; onOpenExample: () => void;
};
const rowClass: Record<Status, string> = { passed: 'step-done', failed: 'step-failed', running: 'step-current', 'needs-you': 'step-current', stale: 'step-waiting', 'not-started': 'step-waiting' };
// Completed rows stay in place; only the active row opens its progress.
function stepRows({ status, stopping, error, modelState = 'idle', firstCheck, initialAutomatic }: Pick<Props, 'status' | 'stopping' | 'error' | 'modelState' | 'firstCheck' | 'initialAutomatic'>) {
  const view = setupView(status, { stopping, model: modelState, check: firstCheck, initialAutomatic });
  const failed = status.localVm === 'failed';
  const paused = status.localVm === 'cancelled' || status.localVm === 'closed';
  const connected = modelState === 'connected';
  const infra = stopping ? null : infraProgress(status);
  const workspace: [Status, string] = status.localVm === 'ready' ? ['passed', 'Ready']
    : failed ? ['failed', 'Needs attention'] : paused ? ['not-started', 'Paused']
    : stopping || status.localVm === 'closing' ? ['running', 'Stopping']
    : infra ? ['running', status.localVm === 'starting' ? 'Starting' : 'Getting ready'] : ['not-started', 'Waiting'];
  const model: [Status, string] = modelState === 'failed' ? ['failed', 'Needs attention'] : connected ? ['passed', 'Connected']
    : modelState === 'connecting' ? ['running', 'Connecting'] : modelState === 'removing' ? ['running', 'Removing sign-in']
    : modelState === 'queued' ? ['not-started', 'Queued']
    : status.localVm === 'ready' ? status.model === 'locked' ? ['not-started', 'Unavailable'] : ['needs-you', 'Needs you']
    : canQueueModel(status) ? ['not-started', 'Pick any time'] : ['not-started', 'Waiting'];
  // A kept result belongs to an earlier workspace until this one is ready with its model connected.
  const test: [Status, string] = firstCheck && status.localVm === 'ready' && connected ? [{ running: 'running', passed: 'passed', failed: 'failed', unknown: 'stale', stale: 'stale' }[firstCheck] as Status,
    { running: 'Running', passed: 'Its test passed', failed: 'Its test failed', unknown: 'Result unknown', stale: 'Needs running' }[firstCheck]]
    : connected ? ['needs-you', 'Needs you'] : ['not-started', 'Waiting'];
  const names = ['Where agents run', 'Get your workspace ready', 'Choose the AI your agent uses', 'Run your first test'];
  // After an unreadable poll the snapshot is last known, not observed: keep the words but nothing runs or is current.
  const rows = [ ['passed', 'On this computer'] as [Status, string], workspace, model, test].map(([state, result], i) => ({ name: names[i], state: error ? 'not-started' as Status : state, result: error ? `Last known: ${result}` : result }));
  return { view, failed, paused, infra, rows };
}

export function SetupProgress(props: Props) {
  const { status, busy, stopping, error, onCancel, onRestart, onCluster, onChange, onOpenExample } = props;
  const [details, setDetails] = useState(false);
  const { view, failed: failedStatus, paused, infra } = stepRows(props);
  const failed = failedStatus && !error;
  const ready = status.localVm === 'ready';
  const progress = status.workspaceProgress;
  const copying = !stopping && status.localVm === 'installing' && progress?.phase === 'materializing';
  const stateLine = error ? 'Local cluster status is unavailable.' : stopping ? 'Stopping the local cluster…' : ready ? 'The local cluster is running on this computer.'
    : paused ? 'Setup is paused. Choose Start again to resume.' : status.localVm === 'starting' ? 'Starting the local cluster…'
    : status.localVm === 'closing' ? 'Stopping the local cluster…'
    : status.localVm === 'notIntegrated' ? 'Local cluster setup is not included in this build. Nothing will be downloaded or started.'
    : copying ? 'Installing the local cluster…' : 'Checking the cluster download…';
  const pill: Status = error ? 'needs-you' : view.status;
  const workspaceDetails = !failed && !error && <>
    {infra && <><div role="progressbar" aria-label="Local cluster setup progress" aria-valuemin={0} aria-valuemax={100}
      aria-valuenow={infra.value === null ? undefined : Math.round(infra.value * 100)} className={`infra-bar${infra.value === null ? ' indeterminate' : ''}`}>
      <span style={infra.value === null ? undefined : { width: `${Math.round(infra.value * 100)}%` }} /></div>
      <p className="muted">{infra.text ?? (status.localVm === 'starting' ? 'Starting the local cluster…' : stateLine)}{infra.remaining && ` · ${infra.remaining} in this file step`}</p></>}
    <Disclosure label={details ? 'Hide details' : 'Show details'} open={details} controls="workspace-progress-details" onOpenChange={setDetails}>
      {details && <div id="workspace-progress-details" className="workspace-progress-details">
        <dl><div><dt>Checking cluster files</dt><dd>{status.localVm === 'preparing' || progress?.phase === 'verifying' ? 'In progress' : ready || copying || status.localVm === 'starting' ? 'Checked' : 'Waiting'}</dd></div>
          <div><dt>Installing cluster files</dt><dd>{ready || status.localVm === 'starting' ? 'Installed' : copying ? 'In progress' : 'Waiting'}</dd></div>
          <div><dt>Starting the local cluster</dt><dd>{ready ? 'Ready' : status.localVm === 'starting' ? 'In progress' : 'Waiting'}</dd></div></dl>
        {progress && status.localVm !== 'starting' && progress.phase !== 'starting' && <p className="muted">{Math.floor(progress.elapsedMs / 1000)} seconds {progress.phase === 'verifying' ? 'checking files' : 'installing files'}.</p>}
        {status.localVmError && <pre tabIndex={0}>{ready ? modelDetails(status.localVmError) : status.localVmError}</pre>}
      </div>}</Disclosure>
  </>;
  return <section className="card workspace-steps setup-progress" aria-label="Local cluster setup">
    {view.done && <div className="steps-head"><h2>Local cluster</h2><StatusPill state={pill}>{pill === 'passed' ? 'Ready' : undefined}</StatusPill><span className="status-description">{error ? 'Unavailable' : view.line}</span></div>}
    <p role="status" className={view.done || paused || error || status.localVm === 'notIntegrated' ? undefined : 'sr-only'}>{view.done || paused || error || ready || stopping || status.localVm === 'notIntegrated' ? stateLine : failed ? 'Setup needs attention' : view.line}</p>
    {ready && !error && props.modelState === 'connected' && props.firstCheck === 'failed' && !!props.firstTestResult?.failed && <p>Its test failed</p>}
    <SetupSteps {...props} workspaceDetails={workspaceDetails} />
    {!failed && <div className="workspace-step-actions">
      {['preparing', 'installing', 'starting'].includes(status.localVm) && <><Button type="button" kind="secondary" disabled={busy || !!error} onClick={onCancel}>Cancel</Button><span className="muted">Cancel pauses setup. Your downloaded files are kept.</span></>}
      {status.localVm === 'notIntegrated' && onCluster && <Button type="button" kind="quiet" disabled={busy} onClick={onCluster}>Use a cluster you already have instead</Button>}
      {paused && <Button type="button" kind="primary" disabled={busy || !!error} onClick={onRestart}>Start again</Button>}
      {ready && <>{props.modelState === 'connected' && <>
        {props.firstCheck === 'stale' ? <Button type="button" kind="primary" disabled={busy || !!error} onClick={onOpenExample}>Run it again</Button>
          : (props.firstCheck === 'unknown' || ((props.firstCheck === 'passed' || props.firstCheck === 'failed') && !props.firstTestResult)) ? <Button type="button" kind="primary" disabled={busy || !!error || !props.onCheckFirstTest} onClick={props.onCheckFirstTest}>Check again</Button>
          : !props.firstCheck && <Button type="button" kind="primary" disabled={busy || !!error} onClick={onOpenExample}>Open your first agent</Button>}
        {props.firstTestResult && <Button type="button" kind={props.firstCheck === 'stale' || props.firstCheck === 'unknown' || props.firstCheck === 'running' ? 'secondary' : 'primary'} disabled={busy || !!error} onClick={onOpenExample}>See what it answered</Button>}
      </>}<Button type="button" kind="quiet" disabled={busy || !!error} onClick={onOpenExample}>Read its instructions</Button><Button type="button" kind="quiet" disabled={busy || !!error} onClick={event => onChange(event.currentTarget)}>Stop the local cluster</Button></>}
    </div>}
  </section>;
}

export function SetupSteps(props: Pick<Props, 'status' | 'busy' | 'stopping' | 'error' | 'modelState' | 'firstCheck' | 'initialAutomatic' | 'onRestart' | 'onCluster'> & { workspaceDetails?: ReactNode }) {
  const { status, busy, error, onRestart, onCluster } = props;
  const { view, failed, rows } = stepRows(props);
  return <ol className="steps-four" aria-label="Setup steps">
      {rows.map(({ name, state, result }, index) => {
        return <li key={name} className={rowClass[state]} aria-current={!view.done && index + 1 === view.step && ['running', 'needs-you'].includes(state) ? 'step' : undefined}>
        <StatusIcon state={state} /><b>{name}</b><span className="step-result">{result}</span>
        {index === 1 && props.workspaceDetails && <div className="step-more">{props.workspaceDetails}</div>}
        {failed && index === 1 && <div className="step-more">
          <Recovery key={status.localVmError} detail={status.localVmError} preflightFailure={status.preflightFailure} failure={status.workspaceFailure} disabled={busy || !!error} onRetry={onRestart} onCluster={onCluster} /></div>}
      </li>; })}
    </ol>;
}

export function Meet({ onRead, onName }: { onRead: () => void; onName?: (name: string | undefined) => void }) {
  const [pair, setPair] = useState<ExamplePair>();
  const [failed, setFailed] = useState(false);
  useEffect(() => { let mounted = true; void loadExample().then(result => { if (!mounted) return; if (result.ok) setPair(result.pair); else setFailed(true); }); return () => { mounted = false; }; }, []);
  const info = agentInfo(pair?.agent);
  useEffect(() => { if (pair) onName?.(info.name); }, [pair, info.name, onName]);
  const webReader = info.name === 'web-reader' && /name: web_fetch(?:\r?\n|$)/.test(pair?.agent ?? '');
  return <section className="card workspace-meet" aria-labelledby="meet-title">
    <span className="workspace-kicker">Meet your first agent</span><h2 id="meet-title">{info.name ?? 'Your first agent'}</h2>
    {pair ? <>{info.description && <p className="meet-description">{info.description}</p>}
      <dl>{webReader && <><div><dt>What it can do</dt><dd><span className="workspace-tool">Read a web page <code>web_fetch</code></span></dd></div>
        <div><dt>How we'll check it</dt><dd>We'll ask for a checksum published on a web page. The check looks for the exact value in the answer.</dd></div></>}
        <div><dt>Whose is it</dt><dd>Yours. You can start a new agent in Designer now. After setup, choose its model and tools and run its checks.</dd></div></dl>
      <Button type="button" kind="secondary" onClick={onRead}>Read its instructions</Button></>
      : <p className="muted">{failed ? 'The example could not be opened. Try opening Designer.' : "Opening the example's introduction…"}</p>}
  </section>;
}
