import { cleanup, render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, test } from 'vitest';
import { DesignerTests } from './DesignerTests';
afterEach(cleanup);
const tests = [{ id: 'page', title: 'Copies the checksum', question: 'What did the page say?' }, { id: 'date', question: 'Which date?' }];
const passed = { id: 'page', title: 'Captured title', question: 'Original question', phase: 'Succeeded', answer: '<script>checksum</script>', completedTools: ['web_fetch'], assertions: [{ id: 'a', kind: 'contains' as const, expected: 'checksum', verdict: 'passed' as const }], verdict: 'passed' as const };
test('tests are recognizable before Run without exposing the source file', async () => {
  const { container } = render(<DesignerTests definitions={tests} stale={false} />);
  expect(screen.getByText('Copies the checksum')).toBeTruthy();
  expect(screen.getByText('page')).toBeTruthy();
  expect(screen.getAllByText('Not started')).toHaveLength(2);
  const row = container.querySelector('summary')!;
  await userEvent.click(row);
  expect(screen.getByText('What did the page say?')).toBeTruthy();
  expect(screen.queryByText('Passed')).toBeNull();
});
test('failed stories open first, passed stories stay folded and show only recorded evidence', async () => {
  const failed = { ...passed, id: 'date', title: 'Says which date', verdict: 'failed' as const, answer: 'No date', assertions: [{ ...passed.assertions[0], expected: '2099', verdict: 'failed' as const }] };
  const { container } = render(<DesignerTests definitions={tests} report={{ verdict: 'failed', cases: [passed, failed] }} stale={false} />);
  const rows = container.querySelectorAll<HTMLDetailsElement>('.test-result');
  expect(rows[0].open).toBe(true); expect(rows[1].open).toBe(false);
  expect(rows[0].textContent).toContain('Says which date');
  expect(screen.getByText('The test failed.')).toBeTruthy();
  await userEvent.click(rows[1].querySelector('summary')!);
  expect(screen.getAllByText('Asked')).toHaveLength(2);
  expect(screen.getAllByText('Used a tool')).toHaveLength(2);
  expect(screen.getAllByText('Read a web page')).toHaveLength(2);
  expect(screen.getByText('<script>checksum</script>')).toBeTruthy();
  expect(container.querySelector('script,a[href]')).toBeNull();
  expect(screen.queryByText('Copies the checksum')).toBeNull();
  expect(screen.getByText('The test passed.')).toBeTruthy();
});
test('recorded tool names cannot resolve inherited object properties', () => {
  const { container } = render(<DesignerTests report={{ verdict: 'failed', cases: [{ ...passed, verdict: 'failed', completedTools: ['__proto__', 'constructor', 'toString'] }] }} stale={false} />);
  expect([...container.querySelectorAll('.story-tools li')].map(node => node.textContent)).toEqual(['__proto__ __proto__', 'constructor constructor', 'toString toString']);
});
test('a cancelled test has a neutral Cancelled pill, never Not started', () => {
  const { container } = render(<DesignerTests report={{ verdict: 'cancelled', cases: [{ ...passed, phase: 'Cancelled', assertions: [], verdict: 'cancelled' }] }} stale={false} />);
  expect(container.querySelector('.test-result > summary .pill')?.textContent).toBe('Cancelled');
  expect(container.querySelector('.test-result > summary .status-not-started')).toBeTruthy();
});
test('missing tool evidence does not claim that a tool was used', () => {
  render(<DesignerTests report={{ verdict: 'failed', cases: [{ ...passed, verdict: 'failed', completedTools: [] }] }} stale={false} />);
  expect(screen.queryByText('Used a tool')).toBeNull();
  expect(screen.getByText('Tool activity')).toBeTruthy();
  expect(screen.getByText('No completed tool call was observed.')).toBeTruthy();
});
test('stale evidence keeps its captured title and never has a current pass pill', () => {
  const { container } = render(<DesignerTests definitions={tests} report={{ verdict: 'passed', cases: [passed] }} stale />);
  expect(screen.getByText('Captured title')).toBeTruthy();
  expect(screen.queryByText('Copies the checksum')).toBeNull();
  expect(container.querySelector('.status-passed')).toBeNull();
  expect(screen.getByText('Earlier result')).toBeTruthy();
});
test('running waits for evidence rather than claiming which task is executing', () => {
  render(<DesignerTests definitions={tests} running stale={false} />);
  expect(screen.getAllByText('Awaiting result')).toHaveLength(2);
  expect(screen.queryByText('Read a web page')).toBeNull();
});
test('suppressed definitions during a running edit are not called malformed files', () => {
  render(<DesignerTests running stale={false} />);
  expect(screen.queryByText(/could not be read/)).toBeNull();
  expect(screen.getByText('Waiting for test results.')).toBeTruthy();
});
test('unsafe title metadata falls back to the id without hiding valid run evidence', () => {
  const title = 'gh' + 'p_' + 'A'.repeat(30);
  render(<DesignerTests report={{ verdict: 'failed', cases: [{ ...passed, title, verdict: 'failed' }] }} stale={false} />);
  expect(screen.queryByText(title)).toBeNull();
  expect(screen.getByText('page')).toBeTruthy();
  expect(screen.getByText('Original question')).toBeTruthy();
});
test('unsupported saved files have an explanation, not an empty What happened section', () => {
  render(<DesignerTests stale={false} />);
  expect(screen.getByText(/Open the test file in Details/)).toBeTruthy();
});
