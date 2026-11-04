import { useEffect, useState } from 'react';
import { engineVersion } from '../engine';

type State = { status: 'loading' }
  | { status: 'available'; version: string; hasDiagnostics: boolean }
  | { status: 'unavailable' | 'error'; message: string };

export default function EngineStatus() {
  const [state, setState] = useState<State>({ status: 'loading' });
  useEffect(() => {
    let active = true;
    void engineVersion().then(result => {
      if (!active) return;
      setState(result.ok
        ? { status: 'available', version: result.value.version, hasDiagnostics: result.value.hasDiagnostics }
        : { status: result.error.kind === 'unavailable' ? 'unavailable' : 'error', message: result.error.message });
    });
    return () => { active = false; };
  }, []);
  return (
    <section className="card engine-status" aria-live="polite">
      <h2>{state.status === 'loading' ? 'Checking engine' : state.status === 'available' ? 'Engine available'
        : state.status === 'unavailable' ? 'Engine unavailable' : 'Engine error'}</h2>
      {state.status === 'available' ? <>
        <p>Version {state.version}. Cluster readiness has not been checked.</p>
        {state.hasDiagnostics && <p className="muted">Engine diagnostics were omitted.</p>}
      </> : <p className="muted">{state.status === 'loading' ? 'Checking engine availability.' : state.message}</p>}
    </section>
  );
}
