import type { RunSnapshot } from '../designer/run-api';
import { validTestTitle, type TestDefinition } from '../designer/test-definitions';
import { type Status, StatusPill } from './DesignSystem';
import { Disclosure } from './Disclosure';
import { type TestCase, TestStory } from './TestStory';

type Props = { definitions?: TestDefinition[]; report?: RunSnapshot['report']; stale: boolean; running?: boolean; vendor?: RunSnapshot['vendor'] };
export function DesignerTests({ definitions, report, stale, running, vendor }: Props) {
  const order = { failed: 0, unknown: 1, passed: 2, cancelled: 3 };
  const rows: { definition: TestDefinition; result?: TestCase }[] = report?.cases.map(result => ({
    definition: { id: result.id, title: validTestTitle(result.title) ? result.title : undefined, question: result.question }, result,
  })).sort((a, b) => order[a.result.verdict] - order[b.result.verdict]) ?? [];
  // Never apply a newly edited title/question to captured evidence.
  if (!report || running && !stale) for (const definition of definitions ?? []) {
    if (!rows.some(row => row.definition.id === definition.id)) rows.push({ definition });
  }
  if (!rows.length) return <p className="muted">{running ? 'Waiting for test results.' : definitions ? 'No test results yet.' : 'The test list could not be read. Open the test file in Details to check its format.'}</p>;
  return <div className={`test-results designer-test-list${stale ? ' result-stale' : ''}`}>
    {rows.map(({ definition: d, result }) => {
      const state: Status = stale && result ? 'stale' : !result ? 'not-started' : result.verdict === 'unknown' ? 'stale' : result.verdict === 'cancelled' ? 'not-started' : result.verdict;
      const label = stale && result ? 'Earlier result' : !result ? running ? 'Awaiting result' : 'Not started' : result.phase === 'NotRun' ? 'Not run' : result.verdict === 'cancelled' ? 'Cancelled' : result.verdict === 'unknown' ? 'Unknown' : undefined;
      return <Disclosure className="test-result" data-result-id={d.id} key={`${d.id}:${result?.verdict ?? 'idle'}:${stale}`} defaultOpen={!!result && result.verdict !== 'passed'} label={<>
        <span className="test-name">{d.title ? <><span>{d.title}</span><code>{d.id}</code></> : <code>{d.id}</code>}</span>
        <StatusPill state={state}>{label}</StatusPill>
      </>}>
        {result ? <TestStory result={result} stale={stale} vendor={vendor} /> : <div className="test-preview"><h5 className="section-label">Question</h5><p className="reading">{d.question}</p></div>}
      </Disclosure>;
    })}
  </div>;
}
