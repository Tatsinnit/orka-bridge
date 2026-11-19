import { useEffect, useRef, useState } from 'react';
import { Button } from '../components/Button';
import { Disclosure } from '../components/Disclosure';
import { retireFirstTest } from './first-test';
import { formatRequirementGB } from './requirements';
import { type UpgradeAction, type UpgradeStatus, upgradeCall } from './upgrade-api';

const failures = {
  network: 'The workspace could not be downloaded. Check your internet connection, then try again. Your old workspace is kept.',
  noSpace: 'There is not enough free space. Free some space, then try again. No retained workspace will be deleted.',
  inUse: 'The workspace files are still in use. Close any other Bridge window, then check again.',
  stopUnconfirmed: 'The workspace could not be stopped safely. Quit and reopen this version of Bridge before trying again.',
  forcedStop: 'The workspace was forcibly stopped without a clean shutdown. Changes that had not reached disk may be lost. Quit and reopen Bridge before trying again.',
  integrity: 'The workspace files could not be verified. Your workspaces are kept. Check again or open the matching Bridge version.',
  unsupported: 'Open the matching Bridge version. This workspace cannot be replaced by this build.',
  unknown: 'The update could not be completed. Your workspaces are kept. Check again before continuing.',
};
export type UpgradeObservationState = { pending: boolean; error?: string; checked?: boolean };
export function UpgradeObservation({ observation }: { observation: UpgradeObservationState }) {
  return <>
    {observation.pending && <p role="status">Checking workspace files. Showing last observed workspace information.</p>}
    {observation.error && <p role="alert">{observation.error}</p>}
    {!observation.pending && observation.error && <p className="muted">Showing last observed workspace information. Workspace files have not been checked again.</p>}
    {!observation.pending && observation.checked && <p role="status">Workspace files checked on request.</p>}
  </>;
}
export default function Upgrade({ status: s, disabled, canUseCredential = true, ordinaryRestartEpoch = 0, observation, onChange, onBusy, onCheck }: {
  observation?: UpgradeObservationState; ordinaryRestartEpoch?: number;
  status: UpgradeStatus; disabled?: boolean; canUseCredential?: boolean; onChange: (status: UpgradeStatus) => void; onBusy: (busy: boolean) => void; onCheck: () => void;
}) {
  const [confirm, setConfirm] = useState<{ action: 'replace' | 'restore' | 'remove'; transaction: string | null; state: UpgradeStatus['state'] }>();
  const [details, setDetails] = useState(false);
  const [working, setWorking] = useState(false);
  const [cancelling, setCancelling] = useState(false);
  const [error, setError] = useState<string>();
  const [outcome, setOutcome] = useState<{ transaction: string | null; ordinaryRestartEpoch: number; message: string }>();
  const [cancelAcknowledgement, setCancelAcknowledgement] = useState<string>();
  const trigger = useRef<HTMLButtonElement>(null);
  const restoreFocus = useRef(false);
  const cancel = useRef<HTMLButtonElement>(null);
  const announcement = useRef<HTMLParagraphElement>(null);
  const mutation = useRef(false);
  const operationEpoch = useRef(0);
  const observedTransaction = useRef(s.transaction);
  useEffect(() => {
    // A cached object for the same active operation is not new cancellation evidence.
    if (observedTransaction.current !== s.transaction || !s.busy && !s.canCancel) {
      ++operationEpoch.current;
      setCancelAcknowledgement(undefined);
    }
    observedTransaction.current = s.transaction;
  }, [s.transaction, s.busy, s.canCancel]);
  const ready = s.state === 'fresh' || s.state === 'consumed';
  function actionAvailable(action: Exclude<UpgradeAction, 'status' | 'refresh' | 'cancel'>) {
    if (action === 'replace' || action === 'retry') return s.canReplace;
    if (action === 'restore') return s.canRestore;
    return ready && !!s.transaction && canUseCredential && (action === 'transfer' ? s.credential === 'eligible' : ['eligible', 'attention', 'transferred'].includes(s.credential));
  }
  const confirmation = confirm?.transaction === s.transaction && confirm.state === s.state && actionAvailable(confirm.action) ? confirm : undefined;
  useEffect(() => {
    if (confirm && !confirmation) { restoreFocus.current = true; setConfirm(undefined); }
  }, [confirm, confirmation]);
  useEffect(() => {
    if (confirmation) cancel.current?.focus();
    else if (restoreFocus.current) {
      restoreFocus.current = false;
      (trigger.current?.isConnected && !trigger.current.disabled ? trigger.current : announcement.current)?.focus();
    }
  }, [confirmation]);
  const busy = s.busy || working;
  const blocked = !!disabled || busy || !!observation?.pending;
  const source = s.sourceReleases.length ? `Bridge ${s.sourceReleases.join(' or ')}` : 'the matching Bridge version';
  const date = s.createdAt === null ? '' : ` from ${new Date(s.createdAt * 1000).toLocaleDateString()}`;
  // Same-transaction polls preserve the result; only an accepted ordinary start supersedes it.
  const finalOutcome = outcome?.transaction === s.transaction && outcome.ordinaryRestartEpoch === ordinaryRestartEpoch ? outcome.message : undefined;
  const retry = ['sourceSelected', 'sourceRetained', 'incomplete'].includes(s.state);
  function ask(action: 'replace' | 'restore' | 'remove', button: HTMLButtonElement) { trigger.current = button; setConfirm({ action, transaction: s.transaction, state: s.state }); }
  function dismiss() { restoreFocus.current = true; setConfirm(undefined); }
  async function cancelUpdate() {
    if (cancelling || !s.canCancel) return;
    setCancelling(true);
    const request = operationEpoch.current;
    try {
      const result = await upgradeCall('cancel');
      if (request !== operationEpoch.current) return;
      if (result.ok) setCancelAcknowledgement(result.outcome === 'requested'
        ? 'Cancellation requested. Waiting for the workspace operation to stop.'
        : result.outcome === 'completed' ? 'Cancellation completed. Waiting for the latest workspace status.'
        : 'This workspace operation can no longer be cancelled. Check the latest status.');
      else setError(result.message);
    } finally { setCancelling(false); }
  }
  async function act(action: Exclude<UpgradeAction, 'status' | 'refresh' | 'cancel'>) {
    if (mutation.current || disabled || s.busy || !actionAvailable(action)) return;
    ++operationEpoch.current;
    mutation.current = true; setWorking(true); setError(undefined); setOutcome(undefined); setCancelAcknowledgement(undefined);
    // Retire before invoking native mutation, not after a possibly lost reply.
    retireFirstTest(); onBusy(true); setConfirm(undefined);
    try {
      const result = await upgradeCall(action, ['retry', 'restore', 'transfer', 'remove'].includes(action) ? s.transaction ?? undefined : undefined);
      if (result.ok) {
        if ('outcome' in result && result.outcome === 'cancelled') setOutcome({ transaction: result.status.transaction, ordinaryRestartEpoch, message: 'Update cancelled. Your retained workspace is kept. Check again before continuing.' });
        onChange(result.status);
      } else setError(result.message);
    } finally { ++operationEpoch.current; setCancelAcknowledgement(undefined); mutation.current = false; setWorking(false); onBusy(false); announcement.current?.focus(); }
  }
  if (s.state === 'none') return null;
  const line = observation?.pending ? 'Last observed workspace information.' : busy ? working || s.canCancel ? 'Updating the workspace. Your agent files are kept.' : 'The workspace is busy. Your agent files are kept.'
    : s.state === 'restored' ? `Previous workspace restored. Quit and open ${source} to use it.`
    : s.state === 'restoring' ? 'Restore was interrupted. Restore the previous workspace to finish recovery.'
    : ready ? 'New workspace selected. Connect the AI explicitly before running your agents.'
    : s.state === 'conflict' ? 'Workspace recovery needs attention. Your workspaces are kept. Open the matching Bridge version or ask for help.'
    : retry ? 'Workspace setup was interrupted. Choose Try setup again to continue. Reopen this version of Bridge if recovery is interrupted again.'
    : 'This version of Bridge needs a new workspace.';
  return <div className="workspace-upgrade">
    <p ref={announcement} tabIndex={-1} role="status">{finalOutcome ?? cancelAcknowledgement ?? line}</p>
    {observation && <UpgradeObservation observation={observation} />}
    {(error || s.error) && <p role="alert">{s.error ? failures[s.error] : error}</p>}
    {!ready && !busy && <p>Your agent files, settings, remembered clusters and saved result summaries are kept. You can still edit files and use a team cluster.</p>}
    {s.retainedAllocatedBytes !== null && <p className="muted">Retained workspace: {formatRequirementGB(s.retainedAllocatedBytes)} allocated. {s.additionalBytes !== null ? `Up to ${formatRequirementGB(s.additionalBytes)} additional space before checking cached files.` : 'Additional space could not be measured yet.'}</p>}
    {s.credential === 'eligible' && (s.state === 'eligible' || ready) && <p>A saved connection can be reused without connecting automatically.</p>}
    {ready && <>
      {s.credential === 'eligible' && !confirmation && <Button type="button" kind="primary" disabled={blocked || !canUseCredential} title={!canUseCredential ? 'Start the workspace before changing its saved connection' : undefined} onClick={() => { void act('transfer'); }}>Use saved connection</Button>}
      {s.credential === 'attention' && <p>The saved connection needs attention. Remove it explicitly before entering another sign-in or key.</p>}
      {s.credential === 'transferred' && <p>Saved connection ready. Use Connect in the AI card; no test or chat will run automatically.</p>}
      {(s.credential === 'absent' || s.credential === 'removed') && <p>Enter a new sign-in or key in the AI card, then choose Connect.</p>}
    </>}
    {confirmation && <fieldset className="workspace-stop-confirm" aria-label={confirmation.action === 'restore' ? 'Restore previous workspace?' : confirmation.action === 'remove' ? 'Remove saved connection?' : 'Set up new workspace?'} onKeyDown={event => { if (event.key === 'Escape' && !busy) { event.stopPropagation(); dismiss(); } }}>
      <legend>{confirmation.action === 'restore' ? 'Restore previous workspace?' : confirmation.action === 'remove' ? 'Remove saved connection?' : 'Set up new workspace?'}</legend>
      {confirmation.action === 'restore' ? <>
        <p>Restore the workspace{date} used with {source}.</p><p>Your current workspace, used with Bridge {s.targetRelease}, will be kept but cannot be reopened by this version’s recovery flow.</p>
        <p>After Restore, quit and open {source}. Removing sign-in in the older app also invalidates the shared saved connection in the retained newer workspace.</p>
      </> : confirmation.action === 'remove' ? <p>This removes Bridge’s saved connection for every retained workspace that references it. Restoring a workspace cannot bring it back. This does not revoke the credential or remove copies an old cluster may have kept.</p> : <>
        <p>Keep the workspace used with {source} and set up a fresh workspace for Bridge {s.targetRelease}. Old cluster objects, chats and volumes stay in the retained workspace; they are not imported.</p>
        <p>If setup is interrupted, reopen this version of Bridge to finish recovery. Do not open the older version until Restore has completed.</p>
        {(s.repeated || s.state === 'restored') && <p>This creates another new workspace. The workspace kept from your last update will not be reopened.</p>}
        <p>Saved results stay historical. Setup will not connect the AI or run a test or chat.</p>
      </>}
      <div className="workspace-step-actions"><Button type="button" kind="secondary" className={confirmation.action === 'remove' ? 'button-danger' : undefined} disabled={blocked} onClick={() => { void act(confirmation.action === 'replace' && retry ? 'retry' : confirmation.action); }}>{confirmation.action === 'restore' ? 'Restore this workspace' : confirmation.action === 'remove' ? 'Remove for all retained workspaces' : 'Keep old workspace and set up new'}</Button>
        <Button ref={cancel} type="button" kind="quiet" disabled={busy} onClick={dismiss}>Not now</Button></div>
    </fieldset>}
    <div hidden={!!confirmation}><div className="workspace-step-actions">
      {s.canReplace && <Button type="button" kind="primary" disabled={blocked} onClick={event => ask('replace', event.currentTarget)}>{retry ? 'Try setup again' : s.repeated || s.state === 'restored' ? 'Set up another new workspace' : 'Set up new workspace'}</Button>}
      {s.canRestore && <Button type="button" kind="secondary" disabled={blocked} onClick={event => ask('restore', event.currentTarget)}>Restore previous workspace</Button>}
      {ready && s.transaction && ['eligible', 'attention', 'transferred'].includes(s.credential) && <Button type="button" kind="quiet" className="button-danger" disabled={blocked || !canUseCredential} title={!canUseCredential ? 'Start the workspace before changing its saved connection' : undefined} onClick={event => ask('remove', event.currentTarget)}>Remove saved connection</Button>}
      {s.canCancel && <Button type="button" kind="secondary" disabled={cancelling} onClick={() => { void cancelUpdate(); }}>Cancel update</Button>}
      {!busy && (s.state === 'conflict' || s.error || error || finalOutcome || observation?.error) && <Button type="button" kind="secondary" disabled={disabled || observation?.pending} onClick={() => { setError(undefined); onCheck(); }}>Check again</Button>}
    </div></div>
    <Disclosure label="Update details" open={details} controls="workspace-upgrade-details" onOpenChange={setDetails}>
      {details && <div id="workspace-upgrade-details"><p>Previous: {source}. New: Bridge {s.targetRelease ?? 'unknown'}.</p>
        {s.transaction && <p>Transaction: <code>{`${s.transaction.slice(0, 4)}…${s.transaction.slice(-4)}`}</code></p>}
        <p>Recovery keeps the newer workspace on disk, but does not offer version switching. Nothing is pruned automatically.</p></div>}
    </Disclosure>
  </div>;
}
