import { Disclosure } from '../components/Disclosure';
import { Button } from '../components/Button';
import { useEffect, useState } from 'react';
import { agentInfo } from '../designer/agent-api';
import { type ExamplePair, loadExample, type SetupStatus } from './state';
import Recovery from './Recovery';
import { StatusIcon, StatusPill, type Status } from '../components/DesignSystem';
import type { ModelState } from './Model';
import { canQueueModel, infraProgress, setupSteps, setupView } from './progress';
import { modelDetails } from './recovery-info';
import './steps.css';

export type CheckStepState = 'running' | 'passed' | 'failed' | 'unknown' | 'stale';

type Props = {
  status: SetupStatus; busy: boolean; stopping: boolean; error?: string; modelState?: ModelState; firstCheck?: CheckStepState;
  firstTestResult?: { failed: number }; initialAutomatic?: boolean; onCheckFirstTest?: () => void;
  onCancel: () => void; onRestart: () => void; onCluster?: () => void; onChange: (button: HTMLButtonElement) => void; onOpenExample: () => void;
};
const rowClass: Record<Status, string> = { passed: 'step-done', failed: 'step-failed', running: 'step-current', 'needs-you': 'step-current', stale: 'step-waiting', 'not-started': 'step-waiting' };
const infraResult = ['Installed', 'Started', 'Ready'];
// The five rows come from the same derivation as the tab and footer.
function stepRows({ status, stopping, error, modelState = 'idle', firstCheck, initialAutomatic }: Pick<Props, 'status' | 'stopping' | 'error' | 'modelState' | 'firstCheck' | 'initialAutomatic'>) {
  const view = setupView(status, { stopping, model: modelState, check: firstCheck, initialAutomatic });
  const failed = status.localVm === 'failed';
  const paused = status.localVm === 'cancelled' || status.localVm === 'closed';
  const connected = modelState === 'connected';
  const infra = stopping ? null : infraProgress(status);
  const infraRow = (n: 1 | 2 | 3): [Status, string] => {
    if (n < view.step) return ['passed', infraResult[n - 1]];
    if (n !== view.step) return ['not-started', 'Waiting'];
    return failed ? ['failed', 'Needs attention'] : paused ? ['not-started', 'Paused'] : ['running', n === 1 ? infra?.remaining ?? '' : 'Starting'];
  };
  const model: [Status, string] = modelState === 'failed' ? ['failed', 'Needs attention'] : connected ? ['passed', 'Connected']
    : modelState === 'connecting' ? ['running', 'Connecting'] : modelState === 'removing' ? ['running', 'Removing sign-in']
    : modelState === 'queued' ? ['not-started', 'Queued']
    : status.localVm === 'ready' ? status.model === 'locked' ? ['not-started', 'Unavailable'] : ['needs-you', 'Needs you']
    : canQueueModel(status) ? ['not-started', 'Pick any time'] : ['not-started', 'Waiting'];
  // A kept result belongs to an earlier workspace until this one is ready with its model connected.
  const test: [Status, string] = firstCheck && status.localVm === 'ready' && connected ? [{ running: 'running', passed: 'passed', failed: 'failed', unknown: 'stale', stale: 'stale' }[firstCheck] as Status,
    { running: 'Running', passed: 'Its test passed', failed: 'Its test failed', unknown: 'Result unknown', stale: 'Needs running' }[firstCheck]]
    : connected ? ['needs-you', 'Needs you'] : ['not-started', 'Waiting'];
  const names = [status.preflightFailure ? 'Check this computer' : 'Install the local cluster', 'Start the local cluster', 'Cluster ready', 'Connect the AI', 'Run your first agent'];
  // After an unreadable poll the snapshot is last known, not observed: keep the words but nothing runs or is current.
  const rows = [infraRow(1), infraRow(2), infraRow(3), model, test].map(([state, result], i) => ({ name: names[i], state: error ? 'not-started' as Status : state, result: error ? `Last known: ${result}` : result }));
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
  // The failed step owns its recovery card; do not render an otherwise empty summary above it.
  if (failed) return null;
  return <section className="card workspace-steps setup-progress" aria-label="Local cluster setup">
    <div className="steps-head"><h2>{view.done ? 'Local cluster' : 'Setting up on this computer'}</h2><StatusPill state={pill}>{pill === 'passed' ? 'Ready' : undefined}</StatusPill><span className="status-description">{error ? 'Unavailable' : !view.done && view.step > 0 && !paused ? `Step ${view.step} of ${setupSteps}` : view.line}</span></div>
    <p className="step-place"><span>Cluster</span><b>Local cluster</b></p>
    {!failed && <p role="status">{stateLine}</p>}
    {ready && !error && props.modelState === 'connected' && props.firstCheck === 'failed' && !!props.firstTestResult?.failed && <p>Its test failed</p>}
    {infra && !failed && !error && <><div role="progressbar" aria-label="Local cluster setup progress" aria-valuemin={0} aria-valuemax={100}
      aria-valuenow={infra.value === null ? undefined : Math.round(infra.value * 100)} className={`infra-bar${infra.value === null ? ' indeterminate' : ''}`}>
      <span style={infra.value === null ? undefined : { width: `${Math.round(infra.value * 100)}%` }} /></div>
      {infra.text && <p className="muted">{infra.text}</p>}{infra.estimate && <p className="muted">{infra.estimate}</p>}</>}
    {!failed && !error && <><Disclosure label={details ? 'Hide details' : 'Show details'} open={details} controls="workspace-progress-details" onOpenChange={setDetails}>
      {details && <div id="workspace-progress-details" className="workspace-progress-details">
        <dl><div><dt>Checking cluster files</dt><dd>{status.localVm === 'preparing' || progress?.phase === 'verifying' ? 'In progress' : ready || copying || status.localVm === 'starting' ? 'Checked' : 'Waiting'}</dd></div>
          <div><dt>Installing cluster files</dt><dd>{ready || status.localVm === 'starting' ? 'Installed' : copying ? 'In progress' : 'Waiting'}</dd></div>
          <div><dt>Starting the local cluster</dt><dd>{ready ? 'Ready' : status.localVm === 'starting' ? 'In progress' : 'Waiting'}</dd></div></dl>
        {progress && <p className="muted">{Math.floor(progress.elapsedMs / 1000)} seconds in this file step.</p>}
        {status.localVmError && <pre tabIndex={0}>{ready ? modelDetails(status.localVmError) : status.localVmError}</pre>}
      </div>}</Disclosure></>}
    <div className="workspace-step-actions">
      {['preparing', 'installing', 'starting'].includes(status.localVm) && <><Button type="button" kind="secondary" disabled={busy || !!error} onClick={onCancel}>Cancel</Button><span className="muted">Cancel pauses setup. Your downloaded files are kept.</span></>}
      {status.localVm === 'notIntegrated' && onCluster && <Button type="button" kind="quiet" disabled={busy} onClick={onCluster}>Use a cluster you already have instead</Button>}
      {paused && <Button type="button" kind="primary" disabled={busy || !!error} onClick={onRestart}>Start again</Button>}
      {ready && <><Button type="button" kind="quiet" disabled={busy || !!error} onClick={onOpenExample}>Read its instructions</Button>{props.modelState === 'connected' && <>
        {props.firstCheck === 'stale' ? <Button type="button" kind="primary" disabled={busy || !!error} onClick={onOpenExample}>Run it again</Button>
          : (props.firstCheck === 'unknown' || ((props.firstCheck === 'passed' || props.firstCheck === 'failed') && !props.firstTestResult)) ? <Button type="button" kind="primary" disabled={busy || !!error || !props.onCheckFirstTest} onClick={props.onCheckFirstTest}>Check again</Button>
          : !props.firstCheck && <Button type="button" kind="primary" disabled={busy || !!error} onClick={onOpenExample}>Open your first agent</Button>}
        {props.firstTestResult && <Button type="button" kind={props.firstCheck === 'stale' || props.firstCheck === 'unknown' || props.firstCheck === 'running' ? 'secondary' : 'primary'} disabled={busy || !!error} onClick={onOpenExample}>See what it answered</Button>}
      </>}<Button type="button" kind="quiet" disabled={busy || !!error} onClick={event => onChange(event.currentTarget)}>Stop the local cluster</Button></>}
    </div>
  </section>;
}

export function SetupSteps(props: Pick<Props, 'status' | 'busy' | 'stopping' | 'error' | 'modelState' | 'firstCheck' | 'initialAutomatic' | 'onRestart' | 'onCluster'>) {
  const { status, busy, error, onRestart, onCluster } = props;
  const { view, failed, rows } = stepRows(props);
  const completed = rows.filter(row => row.state === 'passed');
  const visible = rows.filter((row, index) => failed ? index + 1 === view.step : row.state !== 'passed');
  const list = (items: typeof rows, label = 'Setup steps') => <ol className="steps-four" aria-label={label}>
      {items.map(({ name, state, result }) => {
        const index = rows.findIndex(row => row.name === name);
        return <li key={name} className={rowClass[state]} aria-current={!view.done && index + 1 === view.step && ['running', 'needs-you'].includes(state) ? 'step' : undefined}>
        <StatusIcon state={state} /><b>{name}</b><span className="step-result">{result}</span>
        {failed && index + 1 === view.step && index < 3 && <div className="step-more">
          <Recovery key={status.localVmError} detail={status.localVmError} preflightFailure={status.preflightFailure} failure={status.workspaceFailure} disabled={busy || !!error} onRetry={onRestart} onCluster={onCluster} /></div>}
      </li>; })}
    </ol>;
  return <div className="card setup-history">
    {visible.length > 0 && list(visible)}
    {completed.length > 0 && <Disclosure label="Completed setup steps">{list(completed, 'Completed setup steps')}</Disclosure>}
    {failed && <p className="muted">The remaining setup steps will continue after this is fixed.</p>}
  </div>;
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
