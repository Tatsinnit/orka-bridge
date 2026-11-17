import { useEffect, useId, useRef, useState } from 'react';
import { Button, CloseIcon } from '../components/Button';
import { StatusIcon } from '../components/DesignSystem';
import { Disclosure } from '../components/Disclosure';
import { clusterRecovery } from './recovery';
import { failedCheck, previewServiceAccount, serviceAccounts, type AccountPreview, type ServiceAccounts, type TargetView } from './api';

const labels = { allowed: 'Allowed', denied: 'Denied', unknown: "Couldn't check", notChecked: 'Not checked' };
export default function ServiceAccountPicker({ target, active, disabled, value, onChange }: {
  target: TargetView; active: boolean; disabled: boolean; value: string; onChange: (name: string) => void;
}) {
  const id = useId(); const trigger = useRef<HTMLButtonElement>(null); const input = useRef<HTMLInputElement>(null);
  const listFocus = useRef<Element | null>(null);
  const popover = useRef<HTMLDivElement>(null); const listEpoch = useRef(0); const previewEpoch = useRef(0);
  const [accounts, setAccounts] = useState<ServiceAccounts>(); const [reason, setReason] = useState<string>();
  const [attempt, setAttempt] = useState(0); const [open, setOpen] = useState(false); const [query, setQuery] = useState('');
  const [preview, setPreview] = useState<{ name: string; reply?: AccountPreview; checking?: boolean }>();
  const [details, setDetails] = useState(false);
  function close(focus = true) { setOpen(false); setQuery(''); if (focus) trigger.current?.focus(); }
  useEffect(() => {
    if (!active || disabled || accounts || reason) return;
    const request = ++listEpoch.current; listFocus.current = document.activeElement;
    void serviceAccounts(target.id).then(result => {
      if (request === listEpoch.current) setAccounts(result);
    }, error => {
      if (request !== listEpoch.current) return;
      const failure = failedCheck(error, 'serviceAccount');
      setReason(failure.check === 'serviceAccount' && failure.result === 'permissionDenied' ? `Your current cluster sign-in cannot list ServiceAccounts in ${target.watched_namespace}. Enter an existing account name from your cluster administrator.`
        : failure.result === 'authenticationRejected' ? `Your cluster sign-in was not accepted. ${clusterRecovery(failure).next}`
        : failure.check !== 'serviceAccount' ? "Couldn't recheck this cluster. Check Clusters and try again."
        : "Couldn't list ServiceAccounts. Check the cluster connection and try again.");
    });
    return () => { ++listEpoch.current; };
  }, [target.id, target.watched_namespace, active, disabled, attempt]);
  useEffect(() => {
    if (!active || disabled) { ++previewEpoch.current; setPreview(undefined); setOpen(false); setQuery(''); }
  }, [active, disabled]);
  useEffect(() => () => { ++previewEpoch.current; }, []);
  useEffect(() => {
    const origin = listFocus.current;
    if (active && !disabled && (accounts || reason)
      && (document.activeElement === origin || origin && !origin.isConnected && document.activeElement === document.body)) {
      (accounts?.names.length ? trigger.current : input.current)?.focus();
    }
  }, [accounts, reason, active, disabled]);
  useEffect(() => {
    if (open) { popover.current?.showPopover?.(); popover.current?.querySelector('input')?.focus(); }
  }, [open]);
  async function choose(name: string) {
    close(); onChange(name); setDetails(false);
    const request = ++previewEpoch.current; setPreview({ name, checking: true });
    try {
      const reply = await previewServiceAccount(target.id, name);
      if (request === previewEpoch.current) setPreview({ name, reply });
    } catch { if (request === previewEpoch.current) setPreview({ name }); }
  }
  const result = preview?.name === value ? preview : undefined;
  const status = result?.checking ? 'Checking' : result ? labels[result.reply?.status ?? 'unknown'] : 'Not checked';
  const icon = result?.checking ? 'running' : result?.reply?.status === 'allowed' ? 'passed' : result?.reply?.status === 'denied' ? 'failed' : result ? 'needs-you' : 'not-started';
  const manual = !!reason || accounts?.names.length === 0;
  const matches = accounts?.names.filter(name => name.toLowerCase().includes(query.toLowerCase()));
  return <div className="account-choice">
    {!accounts && !reason ? <p role="status">Listing ServiceAccounts…</p> : manual ? <>
      <p className="muted">{reason ?? (accounts?.truncated ? 'The account list is incomplete. Enter an existing ServiceAccount name.' : `No ServiceAccounts were found in ${target.watched_namespace}.`)}</p>
      <label htmlFor={`${id}-name`}>ServiceAccount name</label>
      <input ref={input} id={`${id}-name`} className="field" value={value} disabled={disabled} autoComplete="off" spellCheck={false} onChange={event => { ++previewEpoch.current; setPreview(undefined); onChange(event.target.value); }} />
      <Button kind="quiet" disabled={disabled} onClick={() => { setAccounts(undefined); setReason(undefined); setAttempt(n => n + 1); }}>Retry account list</Button>
    </> : <>
      <label htmlFor={`${id}-trigger`}>ServiceAccount</label>
      <Button ref={trigger} id={`${id}-trigger`} type="button" kind="secondary" className="context-trigger" aria-label="ServiceAccount" aria-describedby={`${id}-value`} aria-haspopup="dialog" aria-expanded={open} aria-controls={`${id}-picker`} disabled={disabled} onClick={() => { if (open) close(); else setOpen(true); }}>
        <span id={`${id}-value`}>{value ? <code>{value}</code> : 'Choose a ServiceAccount'}</span><span aria-hidden="true">⌄</span>
      </Button>
      {accounts?.truncated && <p className="muted">Showing up to 1,000 accounts. More may exist.</p>}
    </>}
    {accounts?.truncated && !manual && <>
      <label htmlFor={`${id}-name`}>ServiceAccount name</label>
      <input id={`${id}-name`} className="field" value={value} disabled={disabled} autoComplete="off" spellCheck={false} onChange={event => { ++previewEpoch.current; setPreview(undefined); onChange(event.target.value); }} />
    </>}
    {open && active && !disabled && <div ref={popover} popover="auto" role="dialog" aria-label="Choose a ServiceAccount" id={`${id}-picker`} className="context-picker"
      onToggle={event => { if (event.newState === 'closed') close(popover.current?.contains(document.activeElement) === true || document.activeElement === document.body); }}
      onKeyDown={event => { if (event.key === 'Escape') { event.preventDefault(); close(); } }}>
      <Button kind="icon" className="picker-close" aria-label="Close picker" title="Close picker" onClick={() => close()}><CloseIcon /></Button>
      <label htmlFor={`${id}-search`}>Find an account</label>
      <input id={`${id}-search`} type="search" className="field" value={query} onChange={event => setQuery(event.target.value)} />
      <ul>{matches?.map(name => <li key={name}><button type="button" aria-label={name} aria-describedby={`${id}-${name}-status`} aria-current={value === name ? 'true' : undefined} onClick={() => { void choose(name); }}>
        <code>{name}</code><span id={`${id}-${name}-status`} className="muted">{name === value ? status : 'Not checked'}</span>
      </button></li>)}</ul>
      {!matches?.length && <p className="muted">No matching accounts.</p>}
    </div>}
    {value && !manual && <>
      <p role="status" className="cluster-checking"><StatusIcon state={icon} />Required Orka permissions: {status}</p>
      <p className="muted">Name/group preview only. Actual token permissions are checked on Use. Connection and broader access not verified.</p>
      {result?.reply && <Disclosure label="Permission details" open={details} onOpenChange={setDetails}>{details && <ul className="account-permissions">{result.reply.checks.map(check => <li key={`${check.resource}/${check.verb}`}><span>{check.verb} <code>{check.resource}</code></span><span>{labels[check.status]}</span></li>)}</ul>}</Disclosure>}
    </>}
    <p className="muted">Choose an account your administrator provided for Bridge. Required-permission checks do not inspect broader access.</p>
    <p className="muted">Your sign-in: cluster discovery passed. Pod/ReplicaSet access, port-forward and token access: checked on Use.</p>
    <Disclosure label="Discovery access"><p className="muted">Cluster discovery reads Services and Deployments across the cluster and checks cluster-wide API definitions. Account names are listed only in {target.watched_namespace}.</p></Disclosure>
    <p className="muted">Bridge does not create accounts or permissions.</p>
  </div>;
}
