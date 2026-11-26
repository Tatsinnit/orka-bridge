import { type FormEvent, useEffect, useRef, useState } from 'react';
import { Button } from '../components/Button';
import mark from '../assets/orka-mark.svg';

export const welcomePreference = 'bridge.welcomeSeen.v1';
export function hasSeenWelcome() {
  try { return localStorage.getItem(welcomePreference) === '1'; }
  catch { return false; }
}

const starters = ['Summarize a web page', 'Compare two approaches', 'Draft a release checklist'];

export default function Welcome({ titleId, onStart, onCluster, onExample, unavailable }: { titleId?: string; onStart: (prompt: string) => Promise<string | undefined>; onCluster: () => void; onExample: () => void; unavailable?: string }) {
  const [prompt, setPrompt] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string>();
  const field = useRef<HTMLTextAreaElement>(null);
  useEffect(() => { if (error && !busy) field.current?.focus(); }, [busy, error]);
  async function submit(event: FormEvent) {
    event.preventDefault();
    const job = prompt.trim();
    if (!job || busy || unavailable) return;
    setBusy(true); setError(undefined);
    const failure = await onStart(job);
    setBusy(false); setError(failure);
  }
  return <section className="welcome-screen">
    <div className="welcome-topline">
      <span className="welcome-brand"><img className="welcome-mark" src={mark} width="32" height="32" alt="" /><b>Bridge</b></span>
      <Button kind="quiet" type="button" className="welcome-expert" disabled={busy} onClick={onCluster}>I already run Orka on a cluster</Button>
    </div>
    <h1 id={titleId}>What do you want done?</h1>
    <form className="job-intake" onSubmit={event => { void submit(event); }}>
      <label className="sr-only" htmlFor="first-job-prompt">What do you want done?</label>
      <textarea ref={field} id="first-job-prompt" rows={4} maxLength={8192} value={prompt} disabled={busy || !!unavailable}
        onChange={event => { setPrompt(event.currentTarget.value); setError(undefined); }} />
      <div className="job-intake-actions">
        <span className="muted">Runs in a private workspace on this computer.</span>
        <Button kind="primary" type="submit" disabled={busy || !!unavailable || !prompt.trim()}>{busy ? 'Saving job…' : 'Set up and run'} <span aria-hidden="true">→</span></Button>
      </div>
      {error && <p role="alert">{error}</p>}
      {unavailable && <p className="muted">{unavailable}</p>}
    </form>
    <ol className="welcome-journey" aria-label="What happens next">
      <li><span className="journey-number" aria-hidden="true">1</span><b>Set up</b><span>Download and start a private workspace.</span></li>
      <li><span className="journey-number" aria-hidden="true">2</span><b>Connect AI</b><span>Choose GitHub Copilot or your own key.</span></li>
      <li><span className="journey-number" aria-hidden="true">3</span><b>Run your job</b><span>Bridge checks the result before calling it done.</span></li>
    </ol>
    <div className="job-starters"><span className="muted">Or try</span>{starters.map(starter => <Button key={starter} size="small" type="button" disabled={busy || !!unavailable} onClick={() => { setPrompt(starter); setError(undefined); field.current?.focus(); }}>{starter}</Button>)}</div>
    <Button kind="quiet" type="button" disabled={busy} onClick={onExample}>See an example first</Button>
    <p className="welcome-privacy">Nothing leaves this computer during setup. No account is created.</p>
  </section>;
}
