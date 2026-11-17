import { useEffect, useRef, useState } from 'react';
import { Button } from '../components/Button';
import { StatusIcon } from '../components/DesignSystem';
import { Disclosure } from '../components/Disclosure';
import { type ChatCommand, type ChatSnapshot, chatCall, chatErrors } from './chat-api';

const states: Record<ChatSnapshot['state'], string> = {
  ready: 'Ready for your message.', preparing: 'Preparing your saved agent…', working: 'Your agent is working…',
  waitingForInput: 'Your agent requested input that Chat does not support. You can request Stop.',
  waitingForSession: 'Waiting for the conversation to finish saving…',
  observationLost: 'The connection was lost. Reconnect to check what happened; do not resend the message.',
  requestingStop: 'Requesting stop…', stopRequested: 'Stop requested. Execution termination is unconfirmed.',
  stopUnconfirmed: 'Stop request unconfirmed. Execution may still be running.',
  contextUnavailable: 'Earlier messages could not be confirmed. Start a new conversation when available.',
};
export default function Chat({ documentId, blocked, active = true }: { documentId: string; blocked?: string; active?: boolean }) {
  const [snapshot, setSnapshot] = useState<ChatSnapshot>();
  const [draft, setDraft] = useState(''); const draftRef = useRef('');
  const [error, setError] = useState<{ message: string; uncertainSend?: boolean }>(); const [readError, setReadError] = useState<string>();
  const [command, setCommand] = useState<ChatCommand>();
  const mutation = useRef(false); const epoch = useRef(0); const alive = useRef(true);
  const pending = useRef<{ message: string; count: number; uncertain?: boolean } | undefined>(undefined);
  const [awaitingTurn, setAwaitingTurn] = useState(false);
  const composer = useRef<HTMLTextAreaElement>(null); const composing = useRef(false);
  const transcript = useRef<HTMLElement>(null); const scrollArea = useRef<HTMLElement | null>(null); const follow = useRef(true);
  const receive = (value: ChatSnapshot) => {
    const turns = value.versions.flatMap(version => version.turns);
    const submitted = pending.current;
    if (submitted && turns.slice(submitted.count).some(turn => turn.message === submitted.message)) {
      if (draftRef.current === submitted.message) { draftRef.current = ''; setDraft(''); }
      pending.current = undefined; setAwaitingTurn(false);
    } else if (submitted) {
      if (value.busy) submitted.uncertain = false;
      // A lost IPC reply is not a rejection. An idle snapshot alone may precede
      // admission; retain acknowledgement until a turn or its busy→idle is seen.
      if (!value.busy && (value.state === 'contextUnavailable' || !submitted.uncertain && value.state === 'ready')) {
        pending.current = undefined; setAwaitingTurn(false);
      }
    }
    if (submitted && !pending.current) setError(current => current?.uncertainSend ? undefined : current);
    setSnapshot(value);
  };
  const receiveRef = useRef(receive); receiveRef.current = receive;
  useEffect(() => {
    alive.current = true; let mounted = true; let timer: ReturnType<typeof setTimeout>;
    const observe = async () => {
      if (!mounted) return;
      if (!mutation.current) {
        const request = epoch.current;
        const result = await chatCall('agent_chat_status', documentId);
        if (mounted && request === epoch.current) {
          if (result.ok) { receiveRef.current(result.snapshot); setReadError(undefined); } else setReadError(result.message);
        }
      }
      if (mounted) timer = setTimeout(() => { void observe(); }, 500);
    };
    void observe();
    return () => { mounted = false; alive.current = false; epoch.current++; clearTimeout(timer); };
  }, [documentId]);
  useEffect(() => {
    if (!active) return;
    const area = transcript.current?.closest<HTMLElement>('.page-content');
    if (!area) return;
    scrollArea.current = area;
    const track = () => { follow.current = area.scrollHeight - area.clientHeight - area.scrollTop < 48; };
    area.addEventListener('scroll', track);
    return () => { area.removeEventListener('scroll', track); scrollArea.current = null; };
  }, [active]);
  useEffect(() => {
    if (active && snapshot && follow.current && scrollArea.current) scrollArea.current.scrollTop = scrollArea.current.scrollHeight;
  }, [active, snapshot]);
  useEffect(() => {
    if (!active) return;
    const field = composer.current;
    if (!field) return;
    const fit = () => {
      field.style.height = 'auto';
      if (draft) field.style.height = `${field.scrollHeight}px`;
    };
    fit();
    window.addEventListener('resize', fit);
    return () => window.removeEventListener('resize', fit);
  }, [draft, active]);
  const act = async (action: ChatCommand) => {
    if (mutation.current || !snapshot) return;
    // The parent's block covers every conversation reset too; Stop/Reconnect stay available for recovery.
    if (action === 'agent_chat_new' && blocked) return;
    if (action === 'agent_chat_send' && (blocked || !snapshot.canSend || awaitingTurn || !draftRef.current.trim())) return;
    mutation.current = true; const request = ++epoch.current; setCommand(action); setError(undefined);
    if (action === 'agent_chat_send') {
      pending.current = { message: draftRef.current, count: snapshot.versions.reduce((n, version) => n + version.turns.length, 0) };
      setAwaitingTurn(true);
    }
    const operation = action === 'agent_chat_reconnect' && pending.current?.uncertain ? 'agent_chat_status' : action;
    const result = await chatCall(operation, documentId, pending.current?.message);
    if (!alive.current || request !== epoch.current) return;
    if (result.ok) receiveRef.current(result.snapshot);
    else {
      if (action === 'agent_chat_send') {
        if (result.uncertain && pending.current) pending.current.uncertain = true;
        else { pending.current = undefined; setAwaitingTurn(false); }
      }
      setError({ message: result.message, uncertainSend: action === 'agent_chat_send' && result.uncertain });
    }
    mutation.current = false; setCommand(undefined); composer.current?.focus();
  };
  const reason = blocked ?? (command ? 'Wait for this action to finish.' : awaitingTurn ? 'Wait for this message to be accepted.' : !snapshot ? 'Opening your conversation…' : !snapshot.canSend ? 'Wait for the current conversation or test to finish.' : undefined);
  const latestTurn = snapshot?.versions.at(-1)?.turns.at(-1);
  const status = pending.current?.uncertain ? 'Checking whether your message was accepted. Do not resend it.'
    : command === 'agent_chat_stop' ? states.requestingStop
    : snapshot?.state === 'working' && latestTurn && ['', 'Pending', 'Queued', 'Scheduled'].includes(latestTurn.phase) ? 'Waiting to start…'
    : blocked && snapshot?.state === 'ready' ? 'Chat is unavailable.'
    : blocked && snapshot?.state === 'contextUnavailable' ? 'Earlier messages could not be confirmed.'
    : snapshot ? states[snapshot.state] : 'Opening your conversation…';
  const canStop = snapshot && snapshot.state !== 'ready' && snapshot.state !== 'contextUnavailable';
  const canReset = snapshot && !blocked && !snapshot.busy && !awaitingTurn && ['ready', 'contextUnavailable'].includes(snapshot.state);
  const snapshotError = snapshot?.state === 'contextUnavailable' && snapshot.error
    ? blocked ? `New conversation is unavailable: ${blocked}` : 'Use New conversation to continue without unconfirmed history.'
    : snapshot?.state === 'ready' && snapshot.error === 'connection'
    ? 'The workspace could not be reached. Check Clusters, then try sending again.'
    : snapshot?.error ? chatErrors[snapshot.error] : undefined;
  return <section className="chat-panel card" aria-label="Chat on this computer">
    <div className="chat-heading"><h2>Chat on this computer</h2><Button type="button" kind="secondary" disabled={!canReset || !!command} title={blocked ?? (!canReset ? 'Wait for the current turn to finish or reconnect to check it.' : undefined)} onClick={() => { void act('agent_chat_new'); }}>New conversation</Button></div>
    <p className="muted">Talk to your saved agent. Chat does not run tests or change their results.</p>
    <section ref={transcript} className="chat-transcript" aria-label="Conversation">
      {!snapshot?.versions.some(version => version.turns.length) && <p className="muted">Ask your agent a question to start a conversation.</p>}
      {snapshot?.versions.map((version, index) => <section key={index} className="chat-version" aria-label={`Conversation ${index + 1}`}>
        <h3>Conversation {index + 1}</h3>
        {index > 0 && <p className="muted">New conversation. Earlier turns are shown for reference, not sent as history.</p>}
        {version.turns.map((turn, i) => {
          // A terminal Task can be published before the worker finishes reading its result/Session.
          const confirming = turn === latestTurn && (snapshot.state === 'waitingForSession'
            || snapshot.state === 'working' && ['Succeeded', 'Failed', 'Cancelled'].includes(turn.phase));
          const waiting = turn.answer === null && (confirming
            || turn === latestTurn && ['preparing', 'working'].includes(snapshot.state) && ['', 'Pending', 'Queued', 'Scheduled', 'Running', 'Finalizing'].includes(turn.phase));
          return <article className="chat-turn" key={i}>
            <div className="chat-question"><h4>You</h4><p className="reading">{turn.message}</p></div>
            <div className="chat-response"><h4>Agent</h4>
              {turn.phase === 'Failed' && <p>This turn failed.</p>}{turn.phase === 'Cancelled' && <p>The cluster reported this turn cancelled; worker termination is unconfirmed.</p>}
              {turn.answer !== null ? <p className="reading">{turn.answer}</p> : waiting
                ? <p className="chat-waiting"><StatusIcon state="running" />{confirming ? snapshot.state === 'waitingForSession' ? states.waitingForSession : 'Confirming your agent’s reply…' : ['Running', 'Finalizing'].includes(turn.phase) ? 'Waiting for your agent’s reply…' : 'Waiting for your agent to start…'}</p>
                : <p className="muted">{turn.phase === 'Failed' ? 'The turn failed without an answer.' : 'No answer was received.'}</p>}
              {turn.stopUnconfirmed ? <p className="chat-notice">{states.stopUnconfirmed}</p> : turn.terminationUnconfirmed && <p className="chat-notice">{states.stopRequested}</p>}
              {!turn.contextRetained && !waiting && !confirming && <p className="muted">This turn is not confirmed in later context.</p>}
              {!!turn.tools.length && <Disclosure className="card-details" label="Tool activity"><ul className="chat-tools">{turn.tools.map((tool, t) => <li key={t}><code>{tool.name}</code> · {tool.phase}</li>)}</ul></Disclosure>}
            </div>
          </article>;
        })}
      </section>)}
    </section>
    <p role="status" aria-live="polite" aria-atomic="true">{status}</p>
    {(error || readError || snapshotError) && <p role="alert">{error?.message ?? readError ?? snapshotError}</p>}
    <form className="chat-composer" onSubmit={event => { event.preventDefault(); void act('agent_chat_send'); }}>
      <label htmlFor={`message-${documentId}`}>Message</label>
      <textarea className="field" ref={composer} id={`message-${documentId}`} rows={1} value={draft} aria-describedby={`chat-hint-${documentId}`}
        onChange={event => { draftRef.current = event.target.value; setDraft(event.target.value); }}
        onCompositionStart={() => { composing.current = true; }} onCompositionEnd={() => { composing.current = false; }}
        onKeyDown={event => {
          if (event.key === 'Enter' && !event.shiftKey && !composing.current && !event.nativeEvent.isComposing && event.keyCode !== 229) {
            event.preventDefault(); void act('agent_chat_send');
          }
        }} />
      <p className="muted" id={`chat-hint-${documentId}`}>{reason ?? 'Enter to send · Shift+Enter for a new line.'}</p>
      <div className="chat-actions">
        <Button type="submit" kind="primary" disabled={!!reason || !draft.trim()} title={reason ?? (!draft.trim() ? 'Write a message first.' : undefined)}>Send message</Button>
        {canStop && <Button type="button" kind="secondary" disabled={!!command || snapshot.state === 'requestingStop'} onClick={() => { void act('agent_chat_stop'); }}>Stop</Button>}
        {snapshot && snapshot.state !== 'contextUnavailable' && !snapshot.busy && (!snapshot.canSend || pending.current?.uncertain) && <Button type="button" kind="secondary" disabled={!!command} onClick={() => { void act('agent_chat_reconnect'); }}>Reconnect</Button>}
      </div>
    </form>
    <Disclosure className="card-details" label="About this conversation">
      <p>Your agent remembers up to 50 messages. Interrupted replies may not be remembered; tool activity is shown separately.</p>
      <p>This view clears when you quit Bridge. Agents and transcripts remain in the workspace; New conversation does not delete them.</p>
      <p>Stop requests an end to the reply. A tool may still finish while you continue, and its effects are not undone. If the conversation cannot continue, start a new one. You may need to restart the workspace if its history has changed elsewhere.</p>
    </Disclosure>
  </section>;
}
