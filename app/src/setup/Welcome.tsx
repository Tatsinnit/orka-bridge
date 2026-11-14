import { Button } from '../components/Button';
import mark from '../assets/orka-mark.svg';

export const welcomePreference = 'bridge.welcomeSeen.v1';
export function hasSeenWelcome() {
  try { return localStorage.getItem(welcomePreference) === '1'; }
  catch { return false; }
}

export default function Welcome({ titleId, onStart, onCluster, unavailable }: { titleId?: string; onStart: () => void; onCluster: () => void; unavailable?: string }) {
  return <section className="welcome-screen">
    <img className="welcome-mark" src={mark} width="44" height="44" alt="" />
    <h1 id={titleId}>Build an AI agent that does a real job</h1>
    <p className="welcome-lede">Tell it what to do in plain words, watch it work, and check that it got it right. When you're happy, share the same agent with your team. Bridge sets up everything it needs on this computer; you don't need to know Kubernetes.</p>
    <ol className="welcome-journey">
      <li><span className="journey-number" aria-hidden="true">1</span><b>Write it</b><span>Instructions in your own words, and the tools it may use.</span></li>
      <li><span className="journey-number" aria-hidden="true">2</span><b>Test it</b><span>Bridge runs it for real and checks each answer.</span></li>
      <li><span className="journey-number" aria-hidden="true">3</span><b>Share it</b><span>Send the same agent to your team's cluster.</span></li>
    </ol>
    <div className="welcome-actions">
      <Button kind="primary" type="button" onClick={onStart}>Get started</Button>
      {unavailable && <p className="muted">{unavailable}</p>}
    </div>
    <Button kind="quiet" type="button" onClick={onCluster}>I already run Orka on a cluster</Button>
  </section>;
}
