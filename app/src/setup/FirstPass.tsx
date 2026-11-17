import { useEffect, useRef } from 'react';
import { Button } from '../components/Button';
import { Details, StatusIcon } from '../components/DesignSystem';
import { TestStory } from '../components/TestStory';
import type { RunSnapshot } from '../designer/run-api';

// The caller supplies only the source-current result of this setup's own attempt.
export default function FirstPass({ snapshot, onOpen, titleId }: { snapshot: RunSnapshot; onOpen: () => void; titleId?: string }) {
  const heading = useRef<HTMLHeadingElement>(null);
  useEffect(() => { heading.current?.focus(); }, []);
  return <section className="first-pass" aria-labelledby={titleId ?? 'first-pass-title'}>
    <StatusIcon state="passed" />
    <h1 ref={heading} tabIndex={-1} id={titleId ?? 'first-pass-title'}>Your first agent passed its test</h1>
    <p className="muted">Here’s what web-reader just did in your workspace.</p>
    <div className="first-pass-stories">{snapshot.report?.cases.map(result => <TestStory key={result.id} result={result} vendor={snapshot.vendor} />)}</div>
    <p>Instructions, a real tool, and a test that proves it worked. Next, make it yours.</p>
    <div className="workspace-step-actions"><Button type="button" kind="primary" onClick={onOpen}>Open web-reader</Button></div>
    <Details hint="Model and recorded tasks">
      {snapshot.model && <p>Model <code>{snapshot.model}</code></p>}
      {snapshot.acknowledged.filter(receipt => receipt.kind === 'Task').map(receipt => <p key={receipt.uid}>Task <code>{receipt.name}</code></p>)}
    </Details>
  </section>;
}
