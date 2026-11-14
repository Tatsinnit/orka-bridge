import { useEffect, useRef, useState } from 'react';
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
export default function Chat({ documentId, blocked }: { documentId: string; blocked?: string }) {
  const [snapshot, setSnapshot] = useState<ChatSnapshot>();
  const [draft, setDraft] = useState(''); const draftRef = useRef('');
  const [error, setError] = useState<{ message: string; uncertainSend?: boolean }>(); const [readError, setReadError] = useState<string>();
  const [command, setCommand] = useState<ChatCommand>();
  const mutation = useRef(false); const epoch = useRef(0); const alive = useRef(true);
  const pending = useRef<{ message: string; count: number; uncertain?: boolean } | undefined>(undefined);
  const [awaitingTurn, setAwaitingTurn] = useState(false);
  const composer = useRef<HTMLTextAreaElement>(null); const composing = useRef(false);
  const transcript = useRef<HTMLElement>(null); const follow = useRef(true);
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
    if (follow.current && transcript.current) transcript.current.scrollTop = transcript.current.scrollHeight;
  }, [snapshot]);
  const act = async (action: ChatCommand) => {
    if (mutation.current || !snapshot) return;
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
    : snapshot?.state === 'working' && latestTurn && ['', 'Pending', 'Queued'].includes(latestTurn.phase) ? 'Waiting to start…'
    : snapshot ? states[snapshot.state] : 'Opening your conversation…';
  const canStop = snapshot && snapshot.state !== 'ready' && snapshot.state !== 'contextUnavailable';
  const canReset = snapshot && !snapshot.busy && !awaitingTurn && ['ready', 'contextUnavailable'].includes(snapshot.state);
  const snapshotError = snapshot?.state === 'contextUnavailable' && snapshot.error
    ? 'Use New conversation to continue without unconfirmed history.'
    : snapshot?.state === 'ready' && snapshot.error === 'connection'
    ? 'The local cluster could not be reached. Check Clusters, then try sending again.'
    : snapshot?.error ? chatErrors[snapshot.error] : undefined;
  return <section className="chat-panel card" aria-label="Chat on local cluster">
    <div className="chat-heading"><h2>Chat on local cluster</h2><button type="button" className="part-button" disabled={!canReset || !!command} title={!canReset ? 'Wait for the current turn to finish or reconnect to check it.' : undefined} onClick={() => { void act('agent_chat_new'); }}>New conversation</button></div>
    <p className="muted">Talk to your saved agent. Chat does not run tests or change their results.</p>
    <section ref={transcript} className="chat-transcript" aria-label="Conversation" tabIndex={0} onScroll={event => {
      const area = event.currentTarget; follow.current = area.scrollHeight - area.clientHeight - area.scrollTop < 48;
    }}>
      {!snapshot?.versions.some(version => version.turns.length) && <p className="muted">Ask your agent a question to start a conversation.</p>}
      {snapshot?.versions.map((version, index) => <section key={index} className="chat-version" aria-label={`Conversation ${index + 1}`}>
        <h3>Conversation {index + 1}</h3>
        {index > 0 && <p className="muted">New conversation. Earlier turns are shown for reference, not sent as history.</p>}
        {version.turns.map((turn, i) => <article className="chat-turn" key={i}>
          <h4>You</h4><p className="reading">{turn.message}</p>
          <h4>Agent</h4>{turn.phase === 'Failed' && <p>This turn failed.</p>}{turn.phase === 'Cancelled' && <p>The cluster reported this turn cancelled; worker termination is unconfirmed.</p>}{turn.answer !== null ? <p className="reading">{turn.answer}</p> : <p className="muted">{turn.phase === 'Failed' ? 'The turn failed without an answer.' : 'No answer observed yet.'}</p>}
          {turn.stopUnconfirmed ? <p className="chat-notice">{states.stopUnconfirmed}</p> : turn.terminationUnconfirmed && <p className="chat-notice">{states.stopRequested}</p>}
          {!turn.contextRetained && <p className="muted">This turn is not confirmed in later context.</p>}
          {!!turn.tools.length && <details className="card-details"><summary>Tool activity</summary><ul className="chat-tools">{turn.tools.map((tool, t) => <li key={t}><code>{tool.name}</code> · {tool.phase}</li>)}</ul></details>}
        </article>)}
      </section>)}
    </section>
    <p role="status" aria-live="polite" aria-atomic="true">{status}</p>
    {(error || readError || snapshotError) && <p role="alert">{error?.message ?? readError ?? snapshotError}</p>}
    <form className="chat-composer" onSubmit={event => { event.preventDefault(); void act('agent_chat_send'); }}>
      <label htmlFor={`message-${documentId}`}>Message</label>
      <textarea ref={composer} id={`message-${documentId}`} rows={3} value={draft} aria-describedby={`chat-hint-${documentId}`}
        onChange={event => { draftRef.current = event.target.value; setDraft(event.target.value); }}
        onCompositionStart={() => { composing.current = true; }} onCompositionEnd={() => { composing.current = false; }}
        onKeyDown={event => {
          if (event.key === 'Enter' && !event.shiftKey && !composing.current && !event.nativeEvent.isComposing && event.keyCode !== 229) {
            event.preventDefault(); void act('agent_chat_send');
          }
        }} />
      <p className="muted" id={`chat-hint-${documentId}`}>{reason ?? 'Enter to send · Shift+Enter for a new line.'}</p>
      <div className="chat-actions">
        <button type="submit" className="part-button chat-primary" disabled={!!reason || !draft.trim()} title={reason ?? (!draft.trim() ? 'Write a message first.' : undefined)}>Send message</button>
        {canStop && <button type="button" className="part-button" disabled={!!command || snapshot.state === 'requestingStop'} onClick={() => { void act('agent_chat_stop'); }}>Stop</button>}
        {snapshot && snapshot.state !== 'contextUnavailable' && !snapshot.busy && (!snapshot.canSend || pending.current?.uncertain) && <button type="button" className="part-button" disabled={!!command} onClick={() => { void act('agent_chat_reconnect'); }}>Reconnect</button>}
      </div>
    </form>
    <details className="card-details"><summary>About this conversation</summary>
      <p>Orka remembers the latest 50 messages, not unlimited history. Each turn starts a new worker. Tool activity is separate from remembered questions and final answers; an interrupted turn is remembered only when confirmed.</p>
      <p>Conversation history is kept in memory and lost when you quit Bridge. Agents, Sessions and transcripts remain in the local cluster; New conversation does not delete them.</p>
      <p>Stop is best-effort. A tool call may still finish, a later turn may overlap with it, and earlier effects are not undone. A long or externally changed history may need a local cluster restart before continuing.</p>
    </details>
  </section>;
}
