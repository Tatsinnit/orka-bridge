import { Disclosure } from './Disclosure';
import type { SourceId } from '../designer/agent-api';
import type { RunSnapshot } from '../designer/run-api';
import { Details, Elapsed, StatusIcon } from './DesignSystem';
import './test-results.css';

type Report = NonNullable<RunSnapshot['report']>;
type Case = Report['cases'][number];
const words = { passed: 'Passed', failed: 'Failed', unknown: 'Unknown', cancelled: 'Cancelled' };
export function resultLabel(report: Report | null | undefined) {
  if (!report) return undefined;
  if (report.verdict === 'unknown') return 'Result unknown';
  if (report.verdict === 'cancelled') return 'Tests cancelled';
  return `${report.cases.filter(c => c.verdict === 'passed').length} of ${report.cases.length} passed`;
}
export function resultAnnouncement(report: Report | null | undefined) {
  if (!report) return undefined;
  return report.verdict === 'unknown' ? 'Test result unknown.' : `Test result ${report.verdict}. ${resultLabel(report)}.`;
}
export function changedFiles(current: SourceId | undefined, captured: SourceId | null | undefined, unsaved: boolean) {
  if (current && captured) {
    const agent = current.agent !== captured.agent, tests = current.tests !== captured.tests;
    if (agent || tests) return `${agent && tests ? 'Agent and test files' : agent ? 'Agent file' : 'Test file'} changed since this run.`;
  }
  return unsaved ? 'Your agent has unsaved edits.' : 'The current file revision is not confirmed.';
}
export function TestProgress({ stage }: { stage: string }) {
  return <div className="test-progress"><StatusIcon state="running" /><span role="status">{stage}</span><Elapsed /></div>;
}
function reason(c: Case) {
  if (c.phase === 'NotRun') return 'This test was not submitted.';
  if (c.phase === 'Failed') return 'The agent could not finish this test. Try running it again.';
  if (c.phase === 'Cancelled') return 'The test was cancelled before it finished.';
  const failed = c.assertions.filter(a => a.verdict === 'failed');
  if (failed.length) return failed.map(a => a.kind === 'contains'
    ? a.expected.length > 48 ? 'The answer did not include the expected value. Open Details to see it.' : `The answer did not include “${a.expected}”.`
    : `The expected tool call ${a.expected} was not confirmed.`).join(' ');
  const unknown = c.assertions.find(a => a.verdict === 'unknown');
  if (unknown) return unknown.kind === 'toolCalled' ? `A call to ${unknown.expected} was not observed; this result is unknown.` : 'No answer was available to evaluate this test.';
  return undefined;
}
export function TestResults({ report, stale }: { report: Report | null | undefined; stale: boolean }) {
  if (!report) return null;
  const order = { failed: 0, unknown: 1, passed: 2, cancelled: 3 };
  return <div className={stale ? 'test-results result-stale' : 'test-results'}>
    {stale && <p>Earlier result</p>}
    {[...report.cases].sort((a, b) => order[a.verdict] - order[b.verdict]).map(c => <Disclosure className="test-result" key={`${c.id}:${c.verdict}:${stale}`} defaultOpen={c.verdict !== 'passed'} label={<><StatusIcon state={stale ? 'stale' : c.verdict === 'cancelled' ? 'not-started' : c.verdict === 'unknown' ? 'stale' : c.verdict} /><code>{c.id}</code><span>{stale ? 'Earlier result' : c.phase === 'NotRun' ? 'Not run' : words[c.verdict]}</span></>}>
      <div className="test-answer">
        <h5 className="section-label">Question</h5><p className="reading">{c.question}</p>
        <h5 className="section-label">Answer</h5><p className="reading">{c.answer ?? 'No answer was observed.'}</p>
        {reason(c) && <p className="test-reason">{reason(c)}</p>}
        {!!c.completedTools.length && <p>Tools used: {c.completedTools.map((tool, i) => <span key={`${tool}:${i}`}>{i > 0 && ', '}<code>{tool}</code></span>)}</p>}
        <Details hint="Test expectations and execution">
          <p>Execution: {c.phase}</p>
          <ul>{c.assertions.map(a => <li key={a.id}>{a.kind === 'contains' ? <>Answer includes “{a.expected}”</> : <>Uses <code>{a.expected}</code></>}: {{ passed: 'Met', failed: 'Not met', unknown: 'Unknown' }[a.verdict]}</li>)}</ul>
        </Details>
      </div>
    </Disclosure>)}
  </div>;
}
