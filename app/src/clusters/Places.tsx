import type { Ref } from 'react';
import { Button } from '../components/Button';
import { StatusPill } from '../components/DesignSystem';
import type { Place } from '../overview/api';
import type { SetupView } from '../setup/progress';
import type { SetupStatus } from '../setup/state';
import type { ClusterPlaceSnapshot, SelectedPlace } from './api';
import './clusters.css';

export default function Places({ snapshot, unavailable, status, view, place, onLocal, onCluster, recoveryRef, modelRecovery }: {
  recoveryRef?: Ref<HTMLDivElement>; modelRecovery?: boolean;
  snapshot?: ClusterPlaceSnapshot; unavailable: boolean; status?: SetupStatus; view: SetupView; place: SelectedPlace;
  onLocal: () => void; onCluster: (place: Place, selected: boolean) => void;
}) {
  const started = status?.selected === 'localVm';
  const knownEmpty = !unavailable && snapshot?.persistenceState === 'complete' && snapshot.places.length === 0;
  return <div className="places-list">
    <section className="card place-row" aria-label="This computer">
      <span className="icon-tile" aria-hidden="true"><svg className="icon" viewBox="0 0 24 24"><rect x="3" y="4" width="18" height="13" rx="2" /><path d="M8 21h8m-4-4v4" /></svg></span>
      <div className="place-summary"><h2>This computer</h2><p className="place-meta"><span>{!status ? 'Local cluster status could not be read' : status.localVm === 'notIntegrated' ? view.line : started ? view.detail : 'The local cluster runs here'}</span>{(started && view.line !== view.detail || !status) && <span>{view.line}</span>}</p></div>
      {modelRecovery && status?.localVm === 'ready' && <p className="place-workspace-running">Workspace running</p>}
      <div className="place-model-recovery" ref={recoveryRef} />
      <StatusPill state={view.status}>{view.status === 'passed' ? 'Ready' : undefined}</StatusPill>
      <Button type="button" kind={knownEmpty && !started && !!status ? 'primary' : 'secondary'} size="small" onClick={onLocal}>{!status ? 'View details' : view.action}</Button>
    </section>
    {snapshot?.places.map(item => {
      const selected = place.kind === 'cluster' && !!place.target && snapshot.selectedKey === item.key;
      const observed = item.observedAt !== null;
      const checkedAt = item.checkedAt ?? item.observedAt;
      const checked = checkedAt !== null ? `Last checked ${new Date(checkedAt).toLocaleString()}` : 'Not checked yet';
      return <section className="card place-row" aria-label={item.context} key={item.key}>
        <span className="icon-tile" aria-hidden="true"><svg className="icon" viewBox="0 0 24 24"><path d="m12 3 9 5-9 5-9-5 9-5ZM3 13l9 5 9-5M3 18l9 5 9-5" /></svg></span>
        <div className="place-summary"><h2><code>{item.context}</code></h2><p className="place-meta"><span>{item.namespace} · {item.service}</span><span className="muted">{checked}</span>{selected && <span>Selected for Designer</span>}{observed && !item.stale && <span className="muted">{item.agents.length} {item.agents.length === 1 ? 'agent' : 'agents'} at last check</span>}</p></div>
        <StatusPill state={checkedAt !== null ? item.stale ? 'stale' : 'passed' : 'not-started'}>{checkedAt !== null ? item.stale ? 'Stale' : 'Ready' : 'Not checked'}</StatusPill>
        <Button type="button" kind="secondary" size="small" onClick={() => onCluster(item, selected)}>{selected ? 'Manage' : 'Connect'}</Button>
      </section>;
    })}
    {snapshot?.persistenceState === 'unavailable' && <p role="status">Remembered cluster information is unavailable. New connections may not be remembered after Bridge closes.</p>}
    {unavailable ? <p role="status">Couldn't read your clusters. Try opening Clusters again.</p> : !snapshot ? <p role="status">Reading your clusters…</p> : knownEmpty && <p className="muted">No other clusters added yet. Add a Kubernetes cluster with Orka installed.</p>}
  </div>;
}
