import { isAlias, isMap, isNode, isScalar, isSeq, parseDocument, visit, type Node, type YAMLMap } from 'yaml';

// Read ranges, never serialize the document: unknown fields and formatting belong
// to the creator. Unsupported YAML remains available in the File editor.
function document(source: string) {
  const first = source.match(/^---[ \t]*\r?\n/);
  if (!first) return { offset: 0, end: 0, header: '', closed: true };
  const rest = source.slice(first[0].length);
  const close = /^---[ \t]*(?:\r?\n|$)/m.exec(rest);
  if (!close) return { offset: 0, end: 0, header: '', closed: false };
  return { offset: first[0].length, end: first[0].length + close.index + close[0].length, header: rest.slice(0, close.index), closed: true };
}
function metadata(source: string) {
  const split = document(source);
  const doc = parseDocument(split.header, { keepSourceTokens: true });
  let safe = split.closed && split.offset > 0 && !doc.errors.length && isMap(doc.contents);
  visit(doc, (key, node) => {
    if (isNode(node) && (node.tag || ('anchor' in node && node.anchor) || isAlias(node))) safe = false;
    if (key === 'key' && isScalar(node) && node.value === '<<') safe = false;
  });
  return { ...split, root: safe ? doc.contents as YAMLMap : undefined };
}
function text(node: unknown): string | undefined { return isScalar(node) && typeof node.value === 'string' ? node.value : undefined; }
function patch(source: string, start: number, end: number, value: string) { return source.slice(0, start) + value + source.slice(end); }
function newline(source: string) { return source.includes('\r\n') ? '\r\n' : '\n'; }
function profile(root?: YAMLMap) {
  const kmx = root?.get('kmx', true);
  return isMap(kmx) && text(kmx.get('runtime', true)) === 'orka' && kmx.get('schemaVersion') === 1 ? kmx : undefined;
}
export type FormSoul = { inline?: string; configMapRef?: { name: string; key: string }; digest?: string };
function soulForm(kmx?: YAMLMap): { soul?: FormSoul; soulEditable: boolean } {
  const node = kmx?.get('soul', true);
  if (node === undefined) return { soulEditable: !!kmx };
  if (!isMap(node) || node.items.some(item => !['inline', 'configMapRef', 'digest'].includes(String(item.key)))) return { soulEditable: false };
  const inline = text(node.get('inline', true)); const digest = text(node.get('digest', true));
  const ref = node.get('configMapRef', true);
  const validDigest = !node.has('digest') || digest !== undefined;
  if (inline !== undefined && !node.has('configMapRef') && validDigest) return { soul: { inline, ...(digest === undefined ? {} : { digest }) }, soulEditable: true };
  if (!node.has('inline') && isMap(ref) && ref.items.length === 2 && text(ref.get('name', true)) !== undefined && text(ref.get('key', true)) !== undefined && validDigest) {
    return { soul: { configMapRef: { name: text(ref.get('name', true))!, key: text(ref.get('key', true))! }, ...(digest === undefined ? {} : { digest }) }, soulEditable: true };
  }
  return { soulEditable: false };
}
export function readForm(source: string) {
  const meta = metadata(source); const kmx = profile(meta.root); const tools = kmx?.get('tools', true);
  return {
    name: text(meta.root?.get('name', true)), description: text(meta.root?.get('description', true)),
    model: text(meta.root?.get('model', true)),
    modelEditable: !!meta.root && (!meta.root.has('model') || text(meta.root.get('model', true)) !== undefined),
    ...soulForm(kmx), instructions: meta.closed ? source.slice(meta.end) : '',
    instructionsEditable: meta.closed, metadataEditable: !!meta.root,
    descriptionEditable: !!meta.root && (!meta.root.has('description') || text(meta.root.get('description', true)) !== undefined),
    toolsEditable: !!kmx && (tools === undefined || isSeq(tools)),
    tools: isSeq(tools) ? tools.items.map((item, index) => ({ index, name: isMap(item) ? text(item.get('name', true)) : undefined,
      kind: isMap(item) ? text(item.get('kind', true)) : undefined,
      builtin: isMap(item) && text(item.get('kind', true)) === 'builtin' })) : [],
  };
}
export function changeInstructions(source: string, value: string) {
  const split = document(source);
  if (!split.closed || source.slice(split.end).replaceAll('\r\n', '\n') === value.replaceAll('\r\n', '\n')) return source;
  // A Markdown rule at the start of a plain prompt must not become settings.
  const boundary = split.offset === 0 && /^---[ \t]*\r?\n/.test(value) ? `---${newline(source)}---${newline(source)}` : '';
  const separator = split.offset > 0 && !source.slice(0, split.end).endsWith('\n') ? newline(source) : '';
  return source.slice(0, split.end) + boundary + separator + value.replaceAll('\r\n', '\n').replaceAll('\n', newline(source));
}
export function changeDescription(source: string, value: string) {
  const { root, offset } = metadata(source);
  if (!root || !readForm(source).descriptionEditable) return source;
  const node = root.get('description', true);
  if (text(node)?.replaceAll('\r\n', '\n') === value.replaceAll('\r\n', '\n')) return source;
  if (isScalar(node) && node.range) {
    const block = node.type === 'BLOCK_LITERAL' || node.type === 'BLOCK_FOLDED';
    const suffix = block ? (node.comment ? ` #${node.comment}` : '') + newline(source) : '';
    return patch(source, offset + node.range[0], offset + node.range[1], JSON.stringify(value) + suffix);
  }
  return insertField(source, root, offset, `description: ${JSON.stringify(value)}`);
}
function insertField(source: string, map: YAMLMap, offset: number, entry: string) {
  if (!map.range) return source;
  if (map.flow) return patch(source, offset + map.range[0] + 1, offset + map.range[0] + 1, entry + (map.items.length ? ', ' : ''));
  const start = map.range[0]; const absolute = offset + start;
  const indent = absolute - source.lastIndexOf('\n', absolute - 1) - 1;
  return patch(source, absolute, absolute, entry + newline(source) + ' '.repeat(indent));
}
// Both the renderer's YAML 1.2 and native libyaml's YAML 1.1 must decode the
// same bytes. JSON alone leaves these YAML 1.1 line separators unescaped.
function quotedText(value: string) {
  return JSON.stringify(value).replace(/[\u0085\u2028\u2029]/g, character => '\\u' + character.charCodeAt(0).toString(16).padStart(4, '0'));
}
// Replace one scalar only, retaining any trailing comment and block-scalar newline.
function setText(source: string, map: YAMLMap, offset: number, key: string, value: string) {
  const node = map.get(key, true);
  if (node === undefined) return insertField(source, map, offset, `${key}: ${quotedText(value)}`);
  if (!isScalar(node) || !node.range || text(node) === undefined || text(node) === value) return source;
  const block = node.type === 'BLOCK_LITERAL' || node.type === 'BLOCK_FOLDED';
  const suffix = block ? (node.comment ? ` #${node.comment}` : '') + newline(source) : '';
  return patch(source, offset + node.range[0], offset + node.range[1], quotedText(value) + suffix);
}
export function changeModel(source: string, value: string) {
  const meta = metadata(source);
  return readForm(source).modelEditable ? setText(source, meta.root!, meta.offset, 'model', value) : source;
}
export async function changeSoulInline(source: string, value: string) {
  const form = readForm(source);
  if (!form.soulEditable || form.soul?.configMapRef) return source;
  const { root, offset } = metadata(source); const kmx = profile(root)!;
  const soul = kmx.get('soul', true);
  const edited = isMap(soul) ? setText(source, soul, offset, 'inline', value)
    : insertField(source, kmx, offset, `soul: {inline: ${quotedText(value)}}`);
  if (edited === source || form.soul?.digest === undefined) return edited;
  // Hash the decoded replacement and publish the text and digest in one edit.
  const decoded = readForm(edited).soul!.inline!;
  const hash = await crypto.subtle.digest('SHA-256', new TextEncoder().encode(decoded));
  const digest = 'sha256:' + Array.from(new Uint8Array(hash), byte => byte.toString(16).padStart(2, '0')).join('');
  const meta = metadata(edited);
  const replacement = profile(meta.root)?.get('soul', true);
  return isMap(replacement) ? setText(edited, replacement, meta.offset, 'digest', digest) : source;
}
export function changeSoulReference(source: string, field: 'name' | 'key' | 'digest', value: string) {
  const form = readForm(source);
  if (!form.soulEditable || !form.soul?.configMapRef) return source;
  const meta = metadata(source); const soul = profile(meta.root)?.get('soul', true);
  if (!isMap(soul)) return source;
  const map = field === 'digest' ? soul : soul.get('configMapRef', true);
  return isMap(map) ? setText(source, map, meta.offset, field, value) : source;
}
export function addTool(source: string, name: string, kind: 'builtin' | 'tool' = 'builtin') {
  const form = readForm(source);
  if (!form.toolsEditable || form.tools.some(tool => tool.name === name)) return source;
  const { root, offset } = metadata(source); const kmx = profile(root)!; const tools = kmx.get('tools', true);
  const item = `{kind: ${kind}, name: ${JSON.stringify(name)}}`;
  if (tools === undefined) return insertField(source, kmx, offset, `tools: [${item}]`);
  if (!isSeq(tools) || !tools.range) return source;
  const start = offset + tools.range[0];
  if (tools.flow) return patch(source, start + 1, start + 1, item + (tools.items.length ? ',' : ''));
  const indent = start - source.lastIndexOf('\n', start - 1) - 1;
  return patch(source, start, start, `- kind: ${kind}${newline(source)}${' '.repeat(indent + 2)}name: ${JSON.stringify(name)}${newline(source)}${' '.repeat(indent)}`);
}
export function removeTool(source: string, index: number) {
  const form = readForm(source);
  if (!form.toolsEditable || !['builtin', 'tool'].includes(form.tools[index]?.kind ?? '')) return source;
  const { root, offset } = metadata(source); const tools = profile(root)?.get('tools', true);
  if (!isSeq(tools) || !tools.range) return source;
  const item = tools.items[index] as Node;
  if (!item.range) return source;
  if (tools.flow && tools.srcToken?.type === 'flow-collection') {
    const tokens = tools.srcToken.items;
    const comma = (i: number) => tokens[i]?.start.find(token => token.type === 'comma');
    const before = comma(index); const after = comma(index + 1);
    // Delete the separator separately so another entry's comment stays intact.
    const separator = before ?? after;
    if (separator && separator.offset < item.range[0]) {
      return patch(patch(source, offset + item.range[0], offset + item.range[1], ''), offset + separator.offset, offset + separator.offset + 1, '');
    }
    const withoutComma = separator ? patch(source, offset + separator.offset, offset + separator.offset + 1, '') : source;
    return patch(withoutComma, offset + item.range[0], offset + item.range[1], '');
  }
  const token = tools.srcToken?.type === 'block-seq' ? tools.srcToken.items[index].start.find(token => token.type === 'seq-item-ind') : undefined;
  const dash = token?.offset ?? item.range[0];
  const start = source.lastIndexOf('\n', offset + dash - 1) + 1;
  if (tools.items.length === 1) {
    // YAML permits indentless sequences, but not indentless empty flow values.
    return patch(source, offset + tools.range[0], offset + tools.range[1], '  []' + newline(source));
  }
  return patch(source, start, offset + item.range[2], '');
}
