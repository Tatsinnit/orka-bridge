import { expect, test } from 'vitest';
import { readForm, changeDescription, changeInstructions, changeModel, changeSoulInline, changeSoulReference, addTool, removeTool } from './agent-form';
const source = '---\nname: reader\ndescription: \'Read a page\' # keep\nmodel: gpt-file\nfuture: {keep: "exact"} # untouched\nkmx:\n  schemaVersion: 1\n  runtime: orka\n  tools:\n    - kind: builtin\n      name: web_fetch # existing\n    - kind: tool\n      name: custom\n      future: {stay: here}\n---\n\nRead it.\n';
test.each(['\n', '\r\n'])('no-op form leaves all original bytes and scalar styles intact (%j)', nl => {
  const s = source.replaceAll('\n', nl); const form = readForm(s);
  expect(changeDescription(s, form.description!)).toBe(s);
  expect(changeInstructions(s, form.instructions)).toBe(s);
  expect(addTool(s, 'web_fetch')).toBe(s);
});
test.each(['\n', '\r\n'])('model and resource edits preserve unrelated bytes (%j)', nl => {
  const s = source.replaceAll('\n', nl);
  expect(changeModel(s, 'new-model')).toBe(s.replace('model: gpt-file', 'model: "new-model"'));
  expect(addTool(s, 'custom', 'builtin')).toBe(s);
  const added = addTool(s, 'lookup', 'tool');
  expect(readForm(added).tools[0]).toMatchObject({ name: 'lookup', kind: 'tool' });
  expect(removeTool(added, 0)).toBe(s);
});
test('inline soul edit refreshes an existing digest with exact decoded bytes atomically', async () => {
  const digest = 'sha256:' + '0'.repeat(64);
  const s = source.replace('  tools:', `  soul: {inline: old, digest: ${digest}} # keep soul\n  tools:`);
  const changed = await changeSoulInline(s, 'Kind.\r\nExact.');
  expect(readForm(changed).soul).toEqual({ inline: 'Kind.\r\nExact.', digest: 'sha256:7bb41be0bd72dc44d48aad071285146bb37c01b34b84b580313f60499537e7ec' });
  expect(changed).toContain('# keep soul\n');
  expect(changed).toContain(source.slice(source.indexOf('  tools:')));
});
test.each(['\n', '\r\n'])('block scalar and absent soul round-trip without adding an optional digest (%j)', async nl => {
  const s = source.replace('  tools:', '  soul:\n    inline: | # keep\n      Old\n  tools:').replaceAll('\n', nl);
  const changed = await changeSoulInline(s, 'New\nline');
  expect(readForm(changed).soul).toEqual({ inline: 'New\nline' });
  expect(changed).toContain('# keep' + nl);
  expect(changed).toContain('future: {keep: "exact"} # untouched' + nl);
  expect(readForm(await changeSoulInline(source, 'New')).soul).toEqual({ inline: 'New' });
});
test('soul edits escape YAML 1.1 line separators so native Run decodes the same bytes', async () => {
  const s = source.replace('  tools:', '  soul: {inline: old, digest: sha256:old}\n  tools:');
  const changed = await changeSoulInline(s, 'a\u0085b c\u2028 d\u2029e');
  expect(changed).toContain('"a\\u0085b c\\u2028 d\\u2029e"');
  expect(readForm(changed).soul?.inline).toBe('a\u0085b c\u2028 d\u2029e');
});
test('reference edits remain references and unsupported soul shapes are left alone', async () => {
  const s = source.replace('  tools:', '  soul: {configMapRef: {name: persona, key: SOUL.md}, digest: sha256:old} # keep\n  tools:');
  const changed = changeSoulReference(s, 'name', 'persona.v2');
  expect(changed).toBe(s.replace('name: persona', 'name: "persona.v2"'));
  expect(readForm(changeSoulReference(changed, 'key', 'other.md')).soul?.configMapRef?.key).toBe('other.md');
  expect(readForm(changeSoulReference(s, 'digest', 'new')).soul?.digest).toBe('new');
  expect(await changeSoulInline(s, 'Do not switch')).toBe(s);
  const unsupported = source.replace('  tools:', '  soul: {inline: hi, extra: keep}\n  tools:');
  expect(readForm(unsupported).soulEditable).toBe(false);
  expect(await changeSoulInline(unsupported, 'new')).toBe(unsupported);
});
test('Instructions round-trip all whitespace independently of the YAML header', () => {
  for (const body of ['\nRead\n\n', 'Read', '\n\n', 'a\n\nb\n']) {
    const edited = changeInstructions(source, body);
    expect(readForm(edited).instructions).toBe(body);
    expect(edited).toBe(source.slice(0, source.indexOf('---\n\n') + 4) + body);
  }
  expect(changeInstructions('Plain prompt\n', '\nNew prompt\n')).toBe('\nNew prompt\n');
});
test('description edits patch only the scalar, including block scalars', () => {
  expect(changeDescription(source, 'A: new description')).toBe(source.replace("'Read a page'", '"A: new description"'));
  const block = source.replace("'Read a page' # keep\n", '| # keep\n  Read a page\n');
  const changed = changeDescription(block, 'New\nline');
  expect(readForm(changed).description).toBe('New\nline');
  expect(changed).toContain('future: {keep: "exact"} # untouched\n');
});
test('adding and removing a built-in preserve custom tools and untouched bytes', () => {
  const added = addTool(source, 'file_read');
  expect(readForm(added).tools.map(t => t.name)).toEqual(['file_read', 'web_fetch', 'custom']);
  expect(added).toContain('      name: web_fetch # existing\n');
  expect(added).toContain('    - kind: tool\n      name: custom\n      future: {stay: here}\n');
  expect(removeTool(added, 0)).toBe(source);
  expect(readForm(removeTool(source, 1)).tools.map(t => t.name)).toEqual(['web_fetch']);
});
test('flow lists preserve the rest of the profile', () => {
  const s = '---\nkmx: {schemaVersion: 1, runtime: orka, tools: [{kind: builtin, name: web_fetch}], future: yes}\n---\nBody';
  const added = addTool(s, 'file_read');
  expect(readForm(added).tools.map(t => t.name)).toEqual(['file_read', 'web_fetch']);
  expect(removeTool(added, 0)).toBe(s);
  expect(readForm(removeTool(s, 0)).tools).toEqual([]);
});
test('empty and absent tool lists extend only an existing Orka profile', () => {
  for (const fragment of ['  tools: []\n', '']) {
    const s = '---\nkmx:\n  schemaVersion: 1\n  runtime: orka\n' + fragment + 'future: keep\n---\nBody';
    const edited = addTool(s, 'web_fetch');
    expect(readForm(edited).tools[0].name).toBe('web_fetch');
    expect(edited).toContain('future: keep\n');
    expect(readForm(removeTool(edited, 0)).tools).toEqual([]);
  }
  expect(addTool('---\nname: simple\n---\nBody', 'web_fetch')).toBe('---\nname: simple\n---\nBody');
});
test.each(['description: [unfinished', 'description: one\ndescription: two', 'kmx: &profile {tools: []}', 'kmx: *alias', 'description: !custom text', '- not-a-mapping'])('unsupported metadata remains intact: %s', header => {
  const s = `---\n${header}\n---\nBody`;
  expect(readForm(s).metadataEditable).toBe(false);
  expect(changeDescription(s, 'new')).toBe(s);
  expect(addTool(s, 'web_fetch')).toBe(s);
  expect(changeInstructions(s, 'Changed')).toBe(`---\n${header}\n---\nChanged`);
});
test('an EOF closing delimiter gets a newline before the first instruction', () => {
  const s = '---\nname: reader\n---';
  const changed = changeInstructions(s, 'Hi');
  expect(changed).toBe('---\nname: reader\n---\nHi');
  expect(readForm(changed).instructions).toBe('Hi');
  expect(readForm(changed).metadataEditable).toBe(true);
});
test('a headerless prompt can start with a Markdown rule without becoming settings', () => {
  const value = '---\nWrite plainly.\n---\nMore instructions.';
  const changed = changeInstructions('Old instructions', value);
  expect(readForm(changed).instructions).toBe(value);
  expect(readForm(changed).instructionsEditable).toBe(true);
});
test('unclosed front matter is not mistaken for Instructions', () => {
  const s = '---\nname: unfinished';
  expect(readForm(s).instructionsEditable).toBe(false);
  expect(changeInstructions(s, 'replacement')).toBe(s);
});
test('removing an indentless or explicit-dash built-in keeps a valid empty list', () => {
  for (const tools of ['  tools:\n  - kind: builtin\n    name: web_fetch\n', '  tools:\n    -\n      kind: builtin\n      name: web_fetch\n']) {
    const s = '---\nkmx:\n  schemaVersion: 1\n  runtime: orka\n' + tools + '---\nBody';
    const changed = removeTool(s, 0);
    expect(readForm(changed).metadataEditable).toBe(true);
    expect(readForm(changed).tools).toEqual([]);
  }
});
test('removing a later flow built-in retains a custom tool’s inline comment', () => {
  const s = '---\nkmx:\n  schemaVersion: 1\n  runtime: orka\n  tools: [{kind: tool, name: custom}, # keep custom\n    {kind: builtin, name: web_fetch}]\n---\nBody';
  const changed = removeTool(s, 1);
  expect(changed).toContain('# keep custom\n');
  expect(readForm(changed).metadataEditable).toBe(true);
  expect(readForm(changed).tools).toEqual([{ index: 0, name: 'custom', builtin: false, kind: 'tool' }]);
});
test('removing an explicit-dash first item does not leave a null entry', () => {
  const s = '---\nkmx:\n  schemaVersion: 1\n  runtime: orka\n  tools:\n    -\n      kind: builtin\n      name: web_fetch\n    - kind: builtin\n      name: file_read\n---\nBody';
  expect(readForm(removeTool(s, 0)).tools).toEqual([{ index: 0, name: 'file_read', builtin: true, kind: 'builtin' }]);
});
test('a scalar value equal to a merge-key spelling remains editable', () => {
  const changed = changeDescription(source, '<<');
  expect(readForm(changed).description).toBe('<<');
  expect(readForm(changed).descriptionEditable).toBe(true);
  expect(readForm(changeDescription(changed, '<< next')).description).toBe('<< next');
});
test('changing one metadata field leaves mixed newline bytes untouched', () => {
  const s = source.replace('description:', '\r\ndescription:');
  expect(changeDescription(s, 'Changed')).toBe(s.replace("'Read a page'", '"Changed"'));
});
