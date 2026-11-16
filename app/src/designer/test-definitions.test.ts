import { expect, test } from 'vitest';
import { isTestDraft, testDefinitions, validTestTitle } from './test-definitions';
const file = (body: string) => `---\nschemaVersion: 1\n---\n${body}`;
const block = (title = '', id = 'page') => '```yaml agent-test\n' + `id: ${id}\n${title}input: What did the page say?\nassertions: [{id: text, type: contains, value: checksum}]\n` + '```\n';
const draft = '---\nschemaVersion: 1\n---\n\n# helper tests\n\n```yaml agent-test\nid: first-test\ninput: ""\nassertions: []\n```\n';
test('the generated empty skeleton is an incomplete draft, not executable tests', () => {
  for (const source of [draft, draft.replaceAll('\n', '\r\n')]) {
    expect(isTestDraft(source)).toBe(true);
    expect(testDefinitions(source)).toBeUndefined();
  }
});
test.each([
  draft.replace('schemaVersion: 1', 'schemaVersion: 2'),
  draft.replace('input: ""', 'input: Ask something'),
  draft.replace('assertions: []', 'assertions: [{id: answer, type: contains, value: yes}]'),
  draft.replace('id: first-test', 'id: another-test'),
  draft.replace('```yaml', '```yml'),
  draft + '```yaml agent-test\nid: unexpected\n```\n',
])('malformed or edited tests never masquerade as the generated draft', source => {
  expect(isTestDraft(source)).toBe(false);
});
test('saved tests show authored titles with ids, preserving exact decoded text and CRLF', () => {
  expect(testDefinitions(file(block('title: "  é 🙂  "\n') + block('', 'second')).replaceAll('\n', '\r\n'))).toEqual([
    { id: 'page', title: '  é 🙂  ', question: 'What did the page say?' },
    { id: 'second', question: 'What did the page say?' },
  ]);
});
test.each(['null', '42', 'true', '""', '" "', '"a\\nb"', '"a\\u200bb"', JSON.stringify('🙂'.repeat(161)), '"' + 'gh' + 'p_' + 'A'.repeat(30) + '"'])('invalid decoded title %s never creates an idle list', title => {
  expect(testDefinitions(file(block(`title: ${title}\n`)))).toBeUndefined();
});
test('credential boundaries treat non-ASCII letters as word characters, like native Run', () => {
  for (const title of ['Täsk-' + 'A'.repeat(20), 'é' + 'a'.repeat(52)]) expect(validTestTitle(title)).toBe(true);
});
test('title length counts Unicode code points and has the shared credential guard', () => {
  expect(validTestTitle('🙂'.repeat(160))).toBe(true);
  expect(validTestTitle('Host: display label')).toBe(true);
  expect(validTestTitle('sk-' + 'proj-' + 'FAKE')).toBe(false);
});
test.each([
  file(block() + block()), file(block().replace('input:', 'unknown:')), file(block().replace('type: contains', 'type: judge')),
  file(block().replace('id: page', 'id: " "')), file(block().replace('```\n', '')), file('```yml agent-test\nid: hidden\n```\n'),
  file(Array.from({ length: 33 }, (_, i) => block('', `test-${i}`)).join('')),
  file(block().replace('id: page', 'id: page\nid: duplicate')), file(block().replace('assertions:', 'assertions: []\nignored:')),
])('unsupported saved files never fabricate test rows', source => {
  expect(testDefinitions(source)).toBeUndefined();
});
test.each(['42', 'true', 'null', '0x2A', '1.20', 'FALSE', '1e+3', '~'])('native string fields preserve the decoded scalar %s', scalar => {
  const source = file(block('title: Reads the page\n', scalar).replace('input: What did the page say?', `input: ${scalar}`));
  expect(testDefinitions(source)).toEqual([{ id: scalar, title: 'Reads the page', question: scalar }]);
});
test.each(['42', 'true', 'null', '0x2A', '1.20', 'FALSE', '1e+3', '~'])('buffered assertion string fields do not coerce %s, like native Run', scalar => {
  for (const assertion of [`id: ${scalar}, type: contains, value: checksum`, `id: answer, type: contains, value: ${scalar}`, `id: answer, type: toolCalled, tool: ${scalar}`]) {
    expect(testDefinitions(file(block().replace('id: text, type: contains, value: checksum', assertion)))).toBeUndefined();
  }
});
test('bounded YAML aliases accepted by native Run remain available in the preview', () => {
  expect(testDefinitions(file(block('title: &name Reads the page\n').replace('input: What did the page say?', 'input: *name')))?.[0]).toEqual({ id: 'page', title: 'Reads the page', question: 'Reads the page' });
});
test('unrelated fenced code is ignored with the same exact-fence rules as native Run', () => {
  expect(testDefinitions(file('```text\nnot a test\n```\n' + block('', 'real')))?.map(t => t.id)).toEqual(['real']);
  expect(testDefinitions(file('```text\n```yaml agent-test\nid: not-a-test\n```\n' + block('', 'real')))).toBeUndefined();
});
