import { Button } from '../components/Button';
import mark from '../assets/orka-mark.svg';
import { setupTime, type WorkspaceRequirements } from './requirements';

export const welcomePreference = 'bridge.welcomeSeen.v1';
export function hasSeenWelcome() {
  try { return localStorage.getItem(welcomePreference) === '1'; }
  catch { return false; }
}

export default function Welcome({ titleId, onStart, onCluster, unavailable, requirements }: { titleId?: string; onStart: () => void; onCluster: () => void; unavailable?: string; requirements?: WorkspaceRequirements | null }) {
  return <section className="welcome-screen">
    <img className="welcome-mark" src={mark} width="44" height="44" alt="" />
    <h1 id={titleId}>Build an AI agent that does a real job</h1>
    <p className="welcome-lede">Bridge runs your agent on this computer and checks its answers. No Kubernetes needed.</p>
    <ol className="welcome-journey">
      <li><span className="journey-number" aria-hidden="true">1</span><b>Write it</b><span>What it should do and which tools it can use.</span></li>
      <li><span className="journey-number" aria-hidden="true">2</span><b>Test it</b><span>Run it and check each answer.</span></li>
      <li><span className="journey-number" aria-hidden="true">3</span><b>Share it</b><span>Send it to your team's cluster (preview).</span></li>
    </ol>
    <div className="welcome-actions">
      <Button kind="primary" type="button" onClick={onStart}>Get started</Button>
      <span className="muted">{setupTime(requirements)}</span>
      {unavailable && <p className="muted">{unavailable}</p>}
    </div>
    <Button kind="quiet" type="button" className="welcome-expert" onClick={onCluster}>I already run Orka on a cluster</Button>
  </section>;
}
