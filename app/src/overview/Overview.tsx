import { Disclosure } from '../components/Disclosure';
import { Button } from '../components/Button';
import { useEffect, useRef, useState } from 'react';
import { CardSection, Details, GeneratedValue, LongCard, StatusPill, type Status } from '../components/DesignSystem';
import { RelativeTime } from '../components/RelativeTime';
import { sameSource } from '../designer/agent-api';
import { type Agent, type LiveAgents, type Observation, type Places, readAgents, readFiles, readPlaces, type SavedFile, type SavedFiles } from './api';
import { type AgentCard, PlaceCards, RecordedAttempts, remoteCards, SourceGroups } from './places';
import './overview.css';

type Props = { onNewAgent?: () => void; active: boolean; runBusy: boolean; workspaceKey: string; onOpenDocument: (id: string) => void; onOpenExample: () => void; onChooseWorkspace: () => void; onCounts?: (label: string) => void };
type CompleteAgents = LiveAgents & { state: 'complete'; installation: string; observedAt: number };
const complete = (v?: LiveAgents): v is CompleteAgents => v?.state === 'complete' && v.installation !== null && v.observedAt !== null;
const when = (ms: number) => <RelativeTime value={ms} />;
function declarations(file: SavedFile) {
  return file.summary ? <><p>Model in file: {file.summary.model}</p><p>Tools in file: {file.summary.tools.length ? file.summary.tools.join(', ') : 'None'}</p></> : <p>{file.reason ?? 'Saved settings unavailable'}</p>;
}
function identity(agent: Agent, installation: string, observedAt: number) {
  return <dl className="overview-identity"><div><dt>Name</dt><dd>{agent.name}</dd></div><div><dt>Namespace</dt><dd>{agent.namespace}</dd></div><div><dt>UID</dt><dd>{agent.uid}</dd></div><div><dt>Installation</dt><dd>{installation}</dd></div><div><dt>Last checked</dt><dd>{when(observedAt)}</dd></div></dl>;
}
function associated(live: LiveAgents, agent: Agent): Observation | undefined {
  const o = live.observation;
  return o && o.installation === live.installation && o.namespace === agent.namespace && o.agentUid === agent.uid ? o : undefined;
}
export default function Overview({ onNewAgent, active, runBusy, workspaceKey, onOpenDocument, onOpenExample, onChooseWorkspace, onCounts }: Props) {
  const [files, setFiles] = useState<SavedFiles>();
  const [live, setLive] = useState<LiveAgents>();
  const [last, setLast] = useState<CompleteAgents>();
  const [pending, setPending] = useState(false);
  const [refresh, setRefresh] = useState(0);
  const [byAgent, setByAgent] = useState(false);
  const [places, setPlaces] = useState<Places>();
  const [placeError, setPlaceError] = useState(false);
  useEffect(() => {
    if (!active) return;
    let cancelled = false;
    void readPlaces().then(value => {
      if (cancelled) return;
      setPlaceError(!value);
      setPlaces(previous => value ?? (previous && { ...previous, historyState: 'unavailable', places: previous.places.map(p => ({ ...p, stale: true })) }));
    });
    return () => { cancelled = true; };
  }, [active, refresh]);
  const epoch = useRef(0);
  const knownWorkspace = useRef(workspaceKey);
  useEffect(() => {
    const request = ++epoch.current;
    if (knownWorkspace.current !== workspaceKey) {
      knownWorkspace.current = workspaceKey; setLive(undefined); setLast(undefined);
    }
    if (!active) return;
    let timer: ReturnType<typeof setTimeout> | undefined;
    setPending(runBusy);
    async function list(retried = false) {
      const value = await readAgents();
      if (request !== epoch.current) return;
      setLive(value);
      if (complete(value)) { setLast(value); setPending(false); }
      else if (value.state === 'busy' && value.runActive) setPending(true);
      else if (value.state === 'busy' && !retried) timer = setTimeout(() => { void list(true); }, 250);
    }
    void readFiles().then(value => {
      if (request !== epoch.current) return;
      setFiles(value);
      if (runBusy || value.runActive) { setPending(true); return; }
      void list();
    });
    return () => { epoch.current++; clearTimeout(timer); };
  }, [active, runBusy, workspaceKey, refresh]);
  const observed = complete(live) ? live : last;
  const fileLabel = files?.state === 'complete' ? `${files.files.length} saved ${files.files.length === 1 ? 'file' : 'files'}` : 'Saved files unavailable';
  const agentLabel = live?.state === 'complete' && !pending ? `${live.agents.length} ${live.agents.length === 1 ? 'agent' : 'agents'}` : 'Agent count unavailable';
  const clusterCount = places?.places.reduce((sum, p) => sum + p.agents.length, 0) ?? 0;
  const clusterCounts = places?.places.length ? places.places.some(p => p.observedAt === null) ? ' · Cluster agent count unavailable' : ` · ${clusterCount} cluster ${clusterCount === 1 ? 'agent' : 'agents'}${places.places.some(p => p.stale) ? ' (last seen)' : ''}` : '';
  const counts = files ? `${fileLabel} · ${agentLabel}${clusterCounts}` : 'Reading saved files…';
  useEffect(() => { if (active) onCounts?.(counts); }, [active, counts, onCounts]);
  const saved = files?.files ?? [];
  const visibleFiles = saved.filter(file => !observed?.agents.some(agent => {
    const o = associated(observed, agent);
    return o?.documentId === file.documentId && sameSource(file.source ?? undefined, o.source) === true;
  }));
  const allEmpty = files?.state === 'complete' && !saved.length && live?.state === 'complete' && !live.agents.length && !pending;
  const message = pending ? 'Refresh pending until checks finish' : live?.state === 'busy' ? 'Local cluster busy'
    : live?.state === 'unavailable' ? live.reason === 'image' ? 'Agent list unavailable with this Mac image' : live.reason === 'stopped' ? 'Local cluster stopped' : 'Could not read agents'
    : !live ? 'Reading agents…' : 'Local cluster available';
  const localCards: AgentCard[] = observed?.agents.map(agent => {
        const o = associated(observed, agent);
        const file = o && saved.find(f => f.documentId === o.documentId);
        const also = o && places?.historyState === 'complete' ? [...new Set(places.places.filter(p => p.agents.some(a => a.record && sameSource(a.record.source, o.source) === true)).map(p => `${p.context} · ${p.namespace} · ${p.service}`))] : [];
        const sourceUnavailable = !!o && !file?.source;
        const stale = !!o && (sourceUnavailable || sameSource(file?.source ?? undefined, o.source) !== true);
        const state: Status = stale ? 'stale' : o?.state === 'running' ? 'running' : o?.state === 'unknown' || o?.report?.verdict === 'unknown' ? 'stale' : o?.state === 'complete' ? o.report?.verdict ?? 'not-started' : 'not-started';
        const check = sourceUnavailable ? 'Current saved version unavailable' : stale ? 'Checks not run since your change' : o?.state === 'running' ? 'Running checks' : o?.state === 'complete' && o.report && o.report.verdict !== 'unknown' ? `Checks ${o.report.verdict} in this session` : o ? 'Check result unknown' : 'Not tested by Bridge in this session';
        return { key: `${observed.installation}/${agent.namespace}/${agent.uid}`, source: o?.source, place: 'This computer', node: <LongCard key={`${observed.installation}/${agent.namespace}/${agent.uid}`} title={file?.summary?.name || file?.name || agent.name}
          status={<StatusPill state={state} />}
          action={file?.state === 'readable' ? <Button type="button" kind="secondary" size="small" aria-label={`Open ${file.name}`} onClick={() => onOpenDocument(file.documentId)}>Open</Button> : undefined}
          summary={file?.summary?.description ?? 'No linked agent file'}>
          <CardSection title="Checks"><p>{check}</p><p>{o ? <>Last test: {when(o.observedAt)}</> : 'Last run: unavailable'}</p></CardSection>
          {file && <CardSection title="Saved settings">{declarations(file)}</CardSection>}
          {!!also.length && <p className="muted">Also found on {also.join('; ')}</p>}
          <CardSection title="Cluster settings"><p>Model and tools have not been checked.</p></CardSection>
          {file?.state === 'readable' ? <Details hint="Cluster details">{identity(agent, observed.installation, observed.observedAt)}</Details>
            : <Disclosure className="card-details" label="Open"><p>No linked agent file is available to edit.</p>{identity(agent, observed.installation, observed.observedAt)}</Disclosure>}
        </LongCard> };
      }) ?? [];
  const clusterCards = places ? remoteCards(places, saved, localCards, onOpenDocument) : [];
  return <section className="overview" aria-label="Agents by cluster" hidden={!active}>
    <div className="page-heading"><h1 id={active ? 'page-title' : undefined}>Your agents</h1><div className="overview-actions">{onNewAgent && <Button type="button" kind="primary" onClick={onNewAgent}>New agent</Button>}<fieldset className="overview-grouping"><legend className="sr-only">Group agents</legend><Button type="button" kind="secondary" size="small" aria-pressed={!byAgent} onClick={() => setByAgent(false)}>By cluster</Button><Button type="button" kind="secondary" size="small" aria-pressed={byAgent} onClick={() => setByAgent(true)}>By agent</Button></fieldset><Button type="button" kind="secondary" onClick={() => setRefresh(value => value + 1)}>Refresh</Button></div></div>
    {placeError && <p role="status">Could not read clusters</p>}
    {places?.historyState === 'unavailable' && <p role="alert">Saved Send summaries unavailable. Recorded checks cannot be confirmed.</p>}
    <div className={byAgent ? undefined : "overview-board"}><section className="overview-place-section" aria-label="This computer">
    <div className="overview-place"><h2>This computer</h2><p role="status">{message}</p></div>
    <p className="muted overview-counts">{counts}</p>
    {pending && !observed && <p>The local cluster is running tests</p>}
    {observed && (live?.state !== 'complete' || pending) && <p className="muted">Last seen {when(observed.observedAt)}</p>}
    {live?.state === 'unavailable' && live.reason === 'stopped' && <Button type="button" kind="quiet" onClick={onChooseWorkspace}>Open Clusters</Button>}
    {files?.state === 'unavailable' && <p role="alert">Could not read saved files</p>}
    {files?.state === 'complete' && !saved.length && !allEmpty && <p>No saved agent files</p>}
    {allEmpty && <div className="card overview-empty"><h3>No agents on this computer yet</h3><Button type="button" kind="primary" onClick={onOpenExample}>Open example</Button></div>}
    <div className="overview-cards">
      {visibleFiles.map(file => <LongCard key={file.documentId} title={file.summary?.name || file.name} status={<span className="muted">Saved file</span>}
        action={file.state === 'readable' ? <Button type="button" kind="secondary" size="small" aria-label={`Open ${file.name}`} onClick={() => onOpenDocument(file.documentId)}>Open</Button> : undefined}
        summary={file.summary?.description ?? (file.state === 'unreadable' ? 'Saved files unavailable' : 'Agent definition on this computer')}>
        <CardSection title="Saved settings">{declarations(file)}</CardSection>
        <CardSection title="Checks"><p>Check result unavailable for this saved version</p></CardSection>
        {file.source && <Details hint="Saved file hashes"><GeneratedValue label="Agent file hash" value={file.source.agent} /><GeneratedValue label="Check file hash" value={file.source.tests} /></Details>}
      </LongCard>)}
      {!byAgent && localCards.map(card => <div key={card.key}>{card.node}</div>)}
    </div>
    </section>{!byAgent && places && <PlaceCards places={places} cards={clusterCards} />}</div>
    {byAgent && <SourceGroups cards={[...localCards, ...clusterCards]} files={saved} />}
    {places && <RecordedAttempts attempts={places.unobserved} />}
  </section>;
}
