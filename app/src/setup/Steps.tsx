import { type ReactNode, type Ref, useEffect, useState } from 'react';
import { Button } from '../components/Button';
import { type Status, StatusIcon, StatusPill } from '../components/DesignSystem';
import { Disclosure } from '../components/Disclosure';
import { agentInfo } from '../designer/agent-api';
import type { ModelState } from './Model';
import { canQueueModel, infraProgress, setupView } from './progress';
import Recovery from './Recovery';
import { modelDetails } from './recovery-info';
import { type ExamplePair, loadExample, type SetupStatus } from './state';
import './steps.css';
import { readForm } from '../designer/agent-form';
import { builtInTools } from '../designer/built-in-tools';
import { testDefinitions } from '../designer/test-definitions';

export type CheckStepState = 'running' | 'passed' | 'failed' | 'unknown' | 'stale';

type Props = {
  upgrade?: ReactNode; updating?: boolean; updateBlocked?: boolean; ordinaryRecovery?: boolean;
  modelRecoveryRef?: Ref<HTMLDivElement>; modelChoiceRef?: Ref<HTMLDivElement>;
  status: SetupStatus; busy: boolean; stopping: boolean; error?: string; modelState?: ModelState; firstCheck?: CheckStepState;
  modelUnavailable?: boolean;
  job?: boolean;
  firstTestResult?: { failed: number }; initialAutomatic?: boolean; onCheckFirstTest?: () => void; onChooseProvider?: () => void;
  onCancel: () => void; onRestart: () => void; onCluster?: () => void; onOpenExample: () => void;
};
const rowClass: Record<Status, string> = { passed: 'step-done', failed: 'step-failed', running: 'step-current', 'needs-you': 'step-current', stale: 'step-waiting', 'not-started': 'step-waiting' };
// Completed rows stay in place; only the active row opens its progress.
function stepRows({ status, stopping, updating, updateBlocked, error, modelState = 'idle', firstCheck, initialAutomatic, job = false }: Pick<Props, 'status' | 'stopping' | 'updating' | 'updateBlocked' | 'error' | 'modelState' | 'firstCheck' | 'initialAutomatic' | 'job'>) {
  const view = setupView(status, { stopping, model: modelState, check: firstCheck, initialAutomatic, job });
  const failed = status.localVm === 'failed';
  const paused = status.localVm === 'cancelled' || status.localVm === 'closed';
  const connected = modelState === 'connected';
  const infra = stopping ? null : infraProgress(status);
  const workspace: [Status, string] = updating ? ['running', 'Updating'] : updateBlocked ? ['needs-you', 'Needs you'] : stopping || status.localVm === 'closing' ? ['running', 'Stopping']
    : status.localVm === 'ready' ? ['passed', 'Ready']
    : failed ? ['failed', "Couldn't start"] : paused ? ['not-started', 'Paused']
    : infra ? ['running', status.localVm === 'starting' ? 'Starting' : 'Getting ready'] : ['not-started', 'Waiting'];
  const model: [Status, string] = modelState === 'removalFailed' ? ['failed', "Couldn't remove"] : modelState === 'resetFailed' ? ['failed', "Couldn't reset"]
    : modelState === 'failed' ? ['failed', "Couldn't start"] : connected ? ['passed', 'Connected']
    : modelState === 'resetting' ? ['running', 'Resetting'] : modelState === 'connecting' ? ['running', 'Connecting'] : modelState === 'removing' ? ['running', 'Removing sign-in']
    : modelState === 'queued' ? ['not-started', 'Queued']
    : status.localVm === 'ready' ? status.model === 'locked' ? ['not-started', 'Unavailable'] : ['needs-you', 'Needs you']
    : canQueueModel(status) ? ['not-started', 'Pick any time'] : ['not-started', 'Waiting'];
  // A kept result belongs to an earlier workspace until this one is ready with its model connected.
  const test: [Status, string] = firstCheck && status.localVm === 'ready' && connected ? [{ running: 'running', passed: 'passed', failed: 'failed', unknown: 'stale', stale: 'stale' }[firstCheck] as Status,
    { running: 'Running', passed: 'Its test passed', failed: 'Its test failed', unknown: 'Result unknown', stale: 'Needs running' }[firstCheck]]
    : connected ? ['needs-you', 'Needs you'] : ['not-started', 'Waiting'];
  const names = job ? ['Set up', 'Connect the AI', 'Run your job'] : ['Where agents run', 'Get your workspace ready', 'Connect the AI', 'Run your first agent'];
  // After an unreadable poll the snapshot is last known, not observed: keep the words but nothing runs or is current.
  const jobRun: [Status, string] = connected ? ['not-started', 'Runtime needed'] : ['not-started', 'Waiting'];
  const values = job ? [workspace, model, jobRun] : [['passed', 'On this computer'] as [Status, string], workspace, model, test];
  const rows = values.map(([state, result], i) => ({ name: names[i], state: error ? 'not-started' as Status : state, result: error ? `Last known: ${result}` : result }));
  return { view, failed, paused, infra, rows };
}

export function SetupProgress(props: Props) {
  const { status, busy, stopping, error, onCancel, onRestart, onCluster, onOpenExample } = props;
  const [details, setDetails] = useState(false);
  const { view, failed: failedStatus, paused, infra } = stepRows(props);
  const failed = failedStatus && !error;
  const ready = status.localVm === 'ready';
  const needsModel = props.modelState === 'connected' && props.modelUnavailable && props.firstCheck === 'unknown';
  const progress = status.workspaceProgress;
  const copying = !stopping && status.localVm === 'installing' && progress?.phase === 'materializing';
  const stateLine = error ? 'Workspace status is unavailable.' : props.updating ? 'Updating the workspace. Your agent files are kept.' : props.updateBlocked ? 'Workspace recovery needs attention.' : stopping ? 'Stopping the workspace…' : ready ? 'Your workspace is ready on this computer.'
    : paused ? 'Setup is paused. Choose Start again to resume.' : status.localVm === 'starting' ? 'Starting the workspace…'
    : status.localVm === 'closing' ? 'Stopping the workspace…'
    : status.localVm === 'notIntegrated' ? 'Workspace setup is not included in this build. Nothing will be downloaded or started.'
    : copying ? 'Installing the workspace…' : 'Checking the cluster download…';
  const pill: Status = error ? 'needs-you' : props.updating ? 'running' : props.updateBlocked ? 'needs-you' : view.status;
  const workspaceReady = ready && !error && !stopping && !props.updating && !props.updateBlocked;
  const workspaceWord = error ? 'Status unavailable' : props.updating ? 'Updating' : props.updateBlocked ? 'Needs you' : stopping || status.localVm === 'closing' ? 'Stopping' : workspaceReady ? 'Ready' : failed ? "Couldn't start" : undefined;
  const workspaceDetails = props.upgrade ?? (!failed && !error && <>
    {infra && <><div role="progressbar" aria-label="Workspace setup progress" aria-valuemin={0} aria-valuemax={100}
      aria-valuenow={infra.value === null ? undefined : Math.round(infra.value * 100)} className={`infra-bar${infra.value === null ? ' indeterminate' : ''}`}>
      <span style={infra.value === null ? undefined : { width: `${Math.round(infra.value * 100)}%` }} /></div>
      <p className="muted">{infra.text ?? (status.localVm === 'starting' ? 'Starting the workspace…' : stateLine)}</p></>}
    <Disclosure label={details ? 'Hide details' : 'Show details'} open={details} controls="workspace-progress-details" onOpenChange={setDetails}>
      {details && <div id="workspace-progress-details" className="workspace-progress-details">
        <dl><div><dt>Checking cluster files</dt><dd>{status.localVm === 'preparing' || progress?.phase === 'verifying' ? 'In progress' : ready || copying || status.localVm === 'starting' ? 'Checked' : 'Waiting'}</dd></div>
          <div><dt>Installing cluster files</dt><dd>{ready || status.localVm === 'starting' ? 'Installed' : copying ? 'In progress' : 'Waiting'}</dd></div>
          <div><dt>Starting the workspace</dt><dd>{ready ? 'Ready' : status.localVm === 'starting' ? 'In progress' : 'Waiting'}</dd></div></dl>
        {progress && status.localVm !== 'starting' && progress.phase !== 'starting' && <p className="muted">{Math.floor(progress.elapsedMs / 1000)} seconds {progress.phase === 'verifying' ? 'checking files' : 'installing files'}.</p>}
        {status.localVmError && <pre tabIndex={0}>{ready ? modelDetails(status.localVmError) : status.localVmError}</pre>}
      </div>}</Disclosure>
  </>);
  return <section className="card workspace-steps setup-progress" aria-label="Workspace setup">
    <div className="steps-head"><h2>Workspace</h2><StatusPill state={workspaceReady ? 'passed' : pill}>{workspaceWord}</StatusPill></div>
    {view.done && <span className="status-description">{view.line !== 'Workspace ready' ? view.line : undefined}</span>}
    <p role="status" className={paused || error || status.localVm === 'notIntegrated' ? undefined : 'sr-only'}>{view.done || paused || error || ready || stopping || status.localVm === 'notIntegrated' ? stateLine : failed ? 'Setup needs attention' : view.line}</p>
    <SetupSteps {...props} workspaceDetails={workspaceDetails} />
    {props.job && ready && props.modelState === 'connected' && !error && <div className="first-job-runtime" role="status">
      <p><b>Your job is saved.</b> The local coding and review runtime is not included in this build yet, so Bridge has not run it.</p>
      <p className="muted">The workspace and AI connection are ready. You can use the separate example while job execution is being added.</p>
      <Button type="button" kind="secondary" onClick={onOpenExample}>See an example first</Button>
    </div>}
    {ready && !error && props.onChooseProvider && props.modelState === 'connected' && needsModel && <div className="first-test-recovery">
      <p>The first test could not be confirmed. If the AI could not be reached, check your connection or choose another provider.</p>
      <p className="muted">Changing provider first asks you to remove the current sign-in or key. Your agent files are kept.</p>
      <div className="workspace-step-actions"><Button type="button" kind="primary" disabled={busy} onClick={onOpenExample}>Try again in Designer</Button>
        {props.onChooseProvider && <Button type="button" kind="secondary" disabled={busy} onClick={props.onChooseProvider}>Choose another provider</Button>}</div>
    </div>}
    {!failed && <div className="workspace-step-actions">
      {['preparing', 'installing', 'starting'].includes(status.localVm) && <><Button type="button" kind="secondary" disabled={busy || !!error} onClick={onCancel}>Cancel</Button><span className="muted">Cancel pauses setup. Your downloaded files are kept.</span></>}
      {status.localVm === 'notIntegrated' && onCluster && <Button type="button" kind="secondary" disabled={busy} onClick={onCluster}>Use a cluster you already have instead</Button>}
      {paused && (props.ordinaryRecovery ?? !props.upgrade) && <Button type="button" kind="primary" disabled={busy || !!error} onClick={onRestart}>Start again</Button>}
      {ready && !props.job && <>{props.modelState === 'connected' && !needsModel && <>
        {props.firstCheck === 'stale' ? <Button type="button" kind="primary" disabled={busy || !!error} onClick={onOpenExample}>Run it again</Button>
          : (props.firstCheck === 'unknown' || ((props.firstCheck === 'passed' || props.firstCheck === 'failed') && !props.firstTestResult)) ? <Button type="button" kind="primary" disabled={busy || !!error || !props.onCheckFirstTest} onClick={props.onCheckFirstTest}>Check again</Button>
          : !props.firstCheck && <Button type="button" kind="primary" disabled={busy || !!error} onClick={onOpenExample}>Open your first agent</Button>}
        {props.firstTestResult && <Button type="button" kind={props.firstCheck === 'stale' || props.firstCheck === 'unknown' || props.firstCheck === 'running' ? 'secondary' : 'primary'} disabled={busy || !!error} onClick={onOpenExample}>See what it answered</Button>}
      </>}{!needsModel && <Button type="button" kind="secondary" disabled={busy || !!error} onClick={onOpenExample}>Read its instructions</Button>}</>}
    </div>}
  </section>;
}

export function SetupSteps(props: Pick<Props, 'status' | 'busy' | 'stopping' | 'error' | 'modelState' | 'firstCheck' | 'initialAutomatic' | 'onRestart' | 'onCluster' | 'upgrade' | 'updating' | 'updateBlocked' | 'ordinaryRecovery' | 'job'> & { workspaceDetails?: ReactNode; modelRecoveryRef?: Ref<HTMLDivElement>; modelChoiceRef?: Ref<HTMLDivElement> }) {
  const { status, busy, error, onRestart, onCluster } = props;
  const { view, failed, rows } = stepRows(props);
  return <ol className="steps-four" aria-label="Setup steps">
      {rows.map(({ name, state, result }, index) => {
        return <li key={name} className={rowClass[state]} aria-current={!view.done && index + 1 === view.step && ['running', 'needs-you'].includes(state) ? 'step' : undefined}>
        <StatusIcon state={state} /><b>{name}</b><span className="step-result">{result}</span>
        {index === (props.job ? 0 : 1) && props.workspaceDetails && <div className="step-more">{props.workspaceDetails}</div>}
        {index === (props.job ? 1 : 2) && <><div ref={props.modelChoiceRef} className="step-more model-choice-slot" /><div ref={props.modelRecoveryRef} className="step-more model-recovery-slot" /></>}
        {failed && (props.ordinaryRecovery ?? !props.upgrade) && index === (props.job ? 0 : 1) && <div className="step-more">
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
  const form = readForm(pair?.agent ?? '');
  const tools = form.tools.filter((tool): tool is typeof tool & { name: string } => !!tool.name);
  const tests = testDefinitions(pair?.tests ?? '');
  return <section className="card workspace-meet" aria-labelledby="meet-title">
    <span className="workspace-kicker">Meet your first agent</span><h2 id="meet-title">{info.name ?? 'Your first agent'}</h2>
    {pair ? <>{info.description && <p className="meet-description">{info.description}</p>}
      <dl><div><dt>It can use</dt><dd>{tools.length ? tools.map(tool => <span className="workspace-tool" key={tool.index}>{tool.builtin && Object.hasOwn(builtInTools, tool.name) ? builtInTools[tool.name] : 'Tool'} <code>{tool.name}</code></span>) : !form.toolsEditable || tools.length !== form.tools.length ? 'The tool list could not be read. Open its instructions to see the file.' : 'No tools are configured.'}</dd></div>
        {tests && <div><dt>How we'll test it</dt><dd>{tests.map(test => <p key={test.id}>{test.title ?? test.question}</p>)}</dd></div>}
        <div><dt>Whose is it</dt><dd>Yours. After setup, change its instructions and tests, or start a new agent.</dd></div></dl>
      <Button type="button" kind="secondary" onClick={onRead}>Read its instructions</Button></>
      : <p className="muted">{failed ? 'The example could not be opened. Try opening Designer.' : "Opening the example's introduction…"}</p>}
  </section>;
}
