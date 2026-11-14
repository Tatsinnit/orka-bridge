import { Button } from '../components/Button';
import type { ReactNode } from 'react';
import type { AttemptSummary } from '../clusters/send-api';
import { CardSection, Details, GeneratedValue, LongCard, type Status, StatusPill } from '../components/DesignSystem';
import { RelativeTime } from '../components/RelativeTime';
import type { SourceId } from '../designer/agent-api';
import type { Place, Places, RemoteAgent, SavedFile } from './api';

export type AgentCard = { key: string; placeKey?: string; source?: SourceId; place: string; node: ReactNode };
export const sourceKey = (source: SourceId) => `${source.agent}/${source.tests}`;
const when = (ms: number) => <RelativeTime value={ms} />;
const label = (p: Place) => `${p.context} · ${p.namespace} · ${p.service}`;
function ownedFile(source: SourceId, files: SavedFile[]) {
  const matches = files.filter(f => f.state === 'readable' && f.source && sourceKey(f.source) === sourceKey(source));
  return matches.length === 1 ? matches[0] : undefined;
}
function Revision({ source }: { source: SourceId }) {
  return <Details hint="File version"><GeneratedValue label="Agent file hash" value={source.agent} /><GeneratedValue label="Check file hash" value={source.tests} /></Details>;
}
function RemoteCard({ agent, place, historyState, files, also, onOpen }: { agent: RemoteAgent; place: Place; historyState: Places['historyState']; files: SavedFile[]; also: string[]; onOpen: (id: string) => void }) {
  const record = historyState === 'complete' ? agent.record : null;
  const file = record ? ownedFile(record.source, files) : undefined;
  const state: Status = place.stale ? 'stale' : record?.verdict === 'unknown' ? 'not-started' : record?.verdict ?? 'not-started';
  const checks = historyState === 'unavailable' ? 'Recorded checks unavailable' : !record ? 'Not tested by Bridge'
    : place.stale ? `Recorded checks ${record.verdict}; observation stale` : record.verdict === 'unknown' ? 'Checks unknown' : record.verdict === 'running' ? 'Running checks here' : `Checks ${record.verdict} here`;
  const identity = <dl className="overview-identity"><div><dt>Name</dt><dd>{agent.name}</dd></div><div><dt>UID</dt><dd>{agent.uid}</dd></div><div><dt>Cluster</dt><dd>{label(place)}</dd></div><div><dt>Service</dt><dd>{place.service}</dd></div></dl>;
  return <LongCard title={file?.summary?.name || file?.name || agent.name} status={<StatusPill state={state}>agent{place.stale ? ' · stale' : ''}</StatusPill>}
    summary={file?.summary?.description ?? 'Agent without a linked file'}
    action={file ? <Button type="button" kind="secondary" size="small" aria-label={`Open ${file.name}`} onClick={() => onOpen(file.documentId)}>Open</Button> : undefined}>
    <CardSection title="Checks"><p>{checks}</p><p>{record ? <>Last tests: {when(record.time * 1000)}</> : 'Last run: unavailable'}</p></CardSection>
    <CardSection title="Cluster settings"><p>Model on cluster: {agent.model || 'unavailable'}</p><p>Tools on cluster: {agent.tools ? agent.tools.length ? agent.tools.join(', ') : 'None' : 'unavailable'}</p></CardSection>
    {!!also.length && <p className="muted">Also found on {also.join('; ')}</p>}
    {record && <Revision source={record.source} />}
    {file ? <Details hint="Cluster details"><p>Runtime: {agent.agentType || 'unavailable'}</p>{identity}</Details> : <details className="card-details"><summary>Open</summary><p>No single matching agent file is available to edit.</p><p>Runtime: {agent.agentType || 'unavailable'}</p>{identity}</details>}
  </LongCard>;
}
export function remoteCards(places: Places, files: SavedFile[], local: AgentCard[], onOpen: (id: string) => void): AgentCard[] {
  const placements = [...local, ...places.places.flatMap(p => p.agents.map(a => ({ place: label(p), source: places.historyState === 'complete' ? a.record?.source : undefined })))];
  return places.places.flatMap(place => place.agents.map(agent => {
    const source = places.historyState === 'complete' ? agent.record?.source : undefined;
    const also = source ? [...new Set(placements.filter(p => p.place !== label(place) && p.source && sourceKey(p.source) === sourceKey(source)).map(p => p.place))] : [];
    return { key: `${place.key}/${agent.uid}`, placeKey: place.key, source, place: label(place), node: <RemoteCard agent={agent} place={place} files={files} historyState={places.historyState} onOpen={onOpen} also={also} /> };
  }));
}
export function PlaceCards({ places, cards }: { places: Places; cards: AgentCard[] }) {
  return <>{places.places.map(place => <section key={place.key} className="overview-place-section" aria-label={label(place)}>
    <div className="overview-place"><h2>{place.context}</h2><p>{place.namespace} · {place.service}</p></div>
    <p role="status">{place.stale ? 'Earlier cluster check. Refresh to check again.' : `${place.agents.length} ${place.agents.length === 1 ? 'agent' : 'agents'}`}</p>
    {place.observedAt !== null ? <p className="muted">{place.stale ? 'Last seen' : 'Last checked'} {when(place.observedAt)}</p> : <p>Agents not checked yet. Refresh or check access in Clusters.</p>}
    {!place.agents.length && !place.stale && <p>No agents found in this namespace</p>}
    <div className="overview-cards">{cards.filter(c => c.placeKey === place.key).map(c => <div key={c.key}>{c.node}</div>)}</div>
  </section>)}</>;
}
export function SourceGroups({ cards, files }: { cards: AgentCard[]; files: SavedFile[] }) {
  const groups = new Map<string, AgentCard[]>();
  cards.forEach(card => {
    const key = card.source ? sourceKey(card.source) : card.key;
    const group = groups.get(key) ?? []; group.push(card); groups.set(key, group);
  });
  return <div className="overview-board">{[...groups].map(([key, group]) => {
    const source = group[0].source;
    const file = source && ownedFile(source, files);
    const title = source ? `${file?.summary?.name || file?.name || 'Known file'} · saved version` : `Agent without a linked file · ${group[0].place}`;
    return <section key={key} className="overview-place-section" aria-label={title}>
      <div className="overview-place"><h2>{title}</h2></div>
      {source && <Revision source={source} />}
      <div className="overview-cards">{group.map(card => <div key={card.key}><p className="overview-card-place">{card.place}</p>{card.node}</div>)}</div>
    </section>;
  })}</div>;
}
export function RecordedAttempts({ attempts }: { attempts: AttemptSummary[] }) {
  if (!attempts.length) return null;
  return <section className="overview-attempts" aria-label="Earlier Sends"><h2>Earlier Sends</h2><p>Saved Send attempts. Current cluster status is unknown.</p>
    {attempts.map(attempt => <details className="card-details" key={attempt.id}>
      <summary>{attempt.targetDisplay} · Checks {attempt.verdict}</summary>
      <p>{attempt.agentUid ? 'Current agent status unknown' : 'Agent creation not confirmed'}</p>
      <p>Checks {attempt.verdict}</p><p>Last tests: {when(attempt.time * 1000)}</p>
      <p>Planned name: {attempt.plannedAgentName}</p>
      {attempt.agentUid && <p>Recorded UID: {attempt.agentUid}</p>}
      <Revision source={attempt.source} />
    </details>)}
  </section>;
}
