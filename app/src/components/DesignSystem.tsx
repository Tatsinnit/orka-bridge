import { Disclosure } from './Disclosure';
import { Button, CopyIcon } from './Button';
import { type ReactNode, useEffect, useState } from 'react';
import './design-system.css';

export type Status = 'passed' | 'failed' | 'running' | 'stale' | 'needs-you' | 'not-started';
const labels: Record<Status, string> = { passed: 'Passed', failed: 'Failed', running: 'Running', stale: 'Stale', 'needs-you': 'Needs you', 'not-started': 'Not started' };
export function StatusIcon({ state }: { state: Status }) {
  return <svg className={`icon status-icon status-${state}`} viewBox="0 0 24 24" aria-hidden="true">
    <circle cx="12" cy="12" r="9" strokeDasharray={state === 'stale' ? '2 3' : undefined} />
    {state === 'passed' && <path d="m7 12 3 3 7-7" />}
    {state === 'failed' && <path d="m9 9 6 6m0-6-6 6" />}
    {state === 'running' && <path d="M12 3a9 9 0 0 1 0 18Z" fill="currentColor" stroke="none" />}
    {state === 'needs-you' && <circle cx="12" cy="12" r="2" fill="currentColor" stroke="none" />}
  </svg>;
}
export function Elapsed() {
  const [seconds, setSeconds] = useState(0);
  useEffect(() => {
    const start = Date.now();
    const timer = window.setInterval(() => setSeconds(Math.floor((Date.now() - start) / 1000)), 1000);
    return () => window.clearInterval(timer);
  }, []);
  return <small>Observed for {seconds}s</small>;
}
export function StatusPill({ state, children }: { state: Status; children?: ReactNode }) {
  return <span className={`pill status-${state}`}><StatusIcon state={state} />{children ?? labels[state]}</span>;
}
export function Chip({ children }: { children: ReactNode }) { return <span className="chip">{children}</span>; }
export function GeneratedValue({ label, value }: { label: string; value: string }) {
  const [feedback, setFeedback] = useState<{ value: string; text: string }>();
  const colon = value.indexOf(':');
  const kind = colon < 0 ? '' : value.slice(0, colon + 1);
  const raw = value.slice(kind.length);
  const short = raw.length > 8 ? `${kind}${raw.slice(0, 4)}…${raw.slice(-4)}` : value;
  async function copy() {
    try { await navigator.clipboard.writeText(value); setFeedback({ value, text: 'Copied' }); }
    catch { setFeedback({ value, text: 'Could not copy' }); }
  }
  return <div className="generated-value"><span>{label}</span><code title={value}>{short === value ? value : <><span className="sr-only">{value}</span><span aria-hidden="true">{short}</span></>}</code><Button type="button" kind="icon" aria-label={`Copy ${label}`} title={`Copy ${label}`} onClick={() => { void copy(); }}><CopyIcon /></Button><span role="status">{feedback?.value === value ? feedback.text : ''}</span></div>;
}
export function Details({ children, hint }: { children: ReactNode; hint?: string }) {
  return <Disclosure className="card-details" label="Details" hint={hint}><div>{children}</div></Disclosure>;
}
export function CardSection({ title, children }: { title: string; children: ReactNode }) {
  return <section className="card-section" aria-label={title}><h4 className="section-label">{title}</h4>{children}</section>;
}
export function LongCard({ title, status, action, summary, children }: { title: ReactNode; status: ReactNode; action?: ReactNode; summary: ReactNode; children: ReactNode }) {
  return <div className="long-card"><header className="long-card-head"><h3>{title}</h3>{status}<div className="long-card-action">{action}</div></header>{summary && <p className="long-card-summary">{summary}</p>}{children}</div>;
}
