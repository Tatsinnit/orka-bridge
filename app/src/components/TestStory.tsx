import { builtInTools } from '../designer/built-in-tools';
import type { RunSnapshot } from '../designer/run-api';
import { Details, StatusIcon } from './DesignSystem';
import { testReason } from './TestResults';
import './test-story.css';
export type TestCase = NonNullable<RunSnapshot['report']>['cases'][number];
// Frozen native evidence only. A tool name is not proof of its arguments or URL.
export function TestStory({ result, stale = false, vendor }: { result: TestCase; stale?: boolean; vendor?: RunSnapshot['vendor'] }) {
  const reason = testReason(result, vendor);
  const state = stale || result.verdict === 'unknown' ? 'stale' : result.verdict === 'cancelled' ? 'not-started' : result.verdict;
  return <div className="test-answer">
    <ol className="test-story">
      <li><span className="story-number" aria-hidden="true">1</span><div><h5>Asked</h5><p className="reading">{result.question}</p></div></li>
      <li><span className="story-number" aria-hidden="true">2</span><div><h5>{result.completedTools.length ? 'Used a tool' : 'Tool activity'}</h5>{result.completedTools.length
        ? <ul className="story-tools">{result.completedTools.map((tool, i) => <li key={`${tool}:${i}`}>{Object.hasOwn(builtInTools, tool) ? builtInTools[tool] : tool} <code>{tool}</code></li>)}</ul>
        : <p>No completed tool call was observed.</p>}</div></li>
      <li><span className="story-number" aria-hidden="true">3</span><div><h5>Answered</h5><p className="reading">{result.answer ?? 'No answer was observed.'}</p></div></li>
      <li><StatusIcon state={state} /><div><p>{stale ? 'Earlier result, not run since your edit.' : result.phase === 'NotRun' ? 'The test was not submitted.' : { passed: 'The test passed.', failed: 'The test failed.', cancelled: 'The test was cancelled.', unknown: 'The test result is unknown.' }[result.verdict]}</p>{reason && <p className="test-reason">{reason}</p>}</div></li>
    </ol>
    <Details hint="Test expectations and execution">
      <p>Execution: {result.phase}</p>
      {result.verdict === 'cancelled' && <p>Actions already taken are not undone.</p>}
      <ul>{result.assertions.map(a => <li key={a.id}>{a.kind === 'contains' ? <>Answer includes “{a.expected}”</> : <>Uses <code>{a.expected}</code></>}: {{ passed: 'Met', failed: 'Not met', unknown: 'Unknown' }[a.verdict]}</li>)}</ul>
    </Details>
  </div>;
}
