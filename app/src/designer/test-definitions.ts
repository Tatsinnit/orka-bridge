import { parseDocument } from 'yaml';
import patterns from './test-title-patterns.json';
export type TestDefinition = { id: string; title?: string; question: string };
const credentials = patterns.map(pattern => new RegExp(pattern.replace(/^\(\?i\)/, ''), pattern.startsWith('(?i)') ? 'iu' : 'u'));
export function validTestTitle(value: unknown): value is string {
  return typeof value === 'string' && !!value.trim() && [...value].length <= 160
    && !/[\p{Cc}\p{Cf}\p{Zl}\p{Zp}]/u.test(value) && !credentials.some(pattern => pattern.test(value));
}
const text = (value: unknown): value is string => typeof value === 'string' && !!value.trim() && !value.includes('\0');
const object = (value: unknown): value is Record<string, unknown> => !!value && typeof value === 'object' && !Array.isArray(value);
function yaml(source: string, strings = false): unknown {
  const doc = parseDocument(source, { schema: strings ? 'failsafe' : 'core' });
  if (doc.errors.length) throw Error('Unsupported tests');
  return doc.toJS({ maxAliasCount: 100 });
}
// Only the generated empty skeleton is a known draft; execution parsing stays strict.
export function isTestDraft(source: string): boolean {
  return /^---\nschemaVersion: 1\n---\n\n# [a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])? tests\n\n```yaml agent-test\nid: first-test\ninput: ""\nassertions: \[\]\n```\n$/u.test(source.replaceAll('\r\n', '\n'));
}
type Fence = { character: '`' | '~'; length: number; info: string };
function fence(line: string): Fence | undefined {
  const character = line[0];
  if (character !== '`' && character !== '~') return undefined;
  let length = 0;
  while (line[length] === character) length += 1;
  if (length < 3) return undefined;
  const info = line.slice(length);
  if (character === '`' && info.includes('`')) return undefined;
  return { character, length, info };
}
function fenceWhitespace(character: string): boolean {
  const code = character.codePointAt(0);
  return code !== undefined && (code >= 0x0009 && code <= 0x000d || code === 0x0020 || code === 0x0085
    || code === 0x00a0 || code === 0x1680 || code >= 0x2000 && code <= 0x200a || code === 0x2028
    || code === 0x2029 || code === 0x202f || code === 0x205f || code === 0x3000 || code === 0xfeff);
}
function withoutFenceIndent(line: string): string {
  let offset = 0;
  for (const character of line) {
    if (!fenceWhitespace(character)) break;
    offset += character.length;
  }
  return line.slice(offset);
}
// Descriptive only: native Run still validates/captures the saved pair before execution.
// Keep fence recognition identical to engine/run_source.rs.
export function testDefinitions(source: string): TestDefinition[] | undefined {
  if (new TextEncoder().encode(source).length > 1048576) return undefined;
  try {
    const lines = source.split('\n').map(line => line.replace(/\r$/, ''));
    if (lines.shift() !== '---') return undefined;
    const end = lines.indexOf('---');
    if (end < 0) return undefined;
    const header = yaml(lines.splice(0, end + 1).slice(0, -1).join('\n'));
    if (!object(header) || header.schemaVersion !== 1 || Object.keys(header).length !== 1) return undefined;
    const cases: TestDefinition[] = [];
    const ids = new Set<string>();
    let block: string[] | undefined, ordinary: Fence | undefined;
    for (const line of lines) {
      if (block) {
        if (line !== '```') { block.push(line); continue; }
        const caseSource = block.join('\n') + '\n'; block = undefined;
        // Case id/input retain scalar spelling; titles and buffered assertions stay typed.
        const value = yaml(caseSource);
        const strings = yaml(caseSource, true);
        if (!object(value) || !object(strings) || Object.keys(value).some(key => !['id', 'title', 'input', 'assertions'].includes(key))
          || !text(strings.id) || ids.has(strings.id) || !text(strings.input)
          || 'title' in value && !validTestTitle(value.title)
          || !Array.isArray(value.assertions) || !value.assertions.length || value.assertions.length > 64) return undefined;
        const assertions = new Set<string>();
        for (const a of value.assertions) {
          if (!object(a) || !text(a.id) || assertions.has(a.id)
            || !['contains', 'toolCalled'].includes(a.type as string)
            || Object.keys(a).sort().join(',') !== (a.type === 'contains' ? 'id,type,value' : 'id,tool,type')
            || !text(a.type === 'contains' ? a.value : a.tool)) return undefined;
          assertions.add(a.id);
        }
        ids.add(strings.id);
        cases.push({ id: strings.id, question: strings.input, ...('title' in value ? { title: value.title as string } : {}) });
        if (cases.length > 32) return undefined;
        continue;
      }
      if (ordinary) {
        const close = fence(line);
        if (close?.character === ordinary.character && close.length >= ordinary.length && [...close.info].every(fenceWhitespace)) ordinary = undefined;
        continue;
      }
      if (line === '```yaml agent-test') { block = []; continue; }
      if (fence(withoutFenceIndent(line))?.info.includes('agent-test')) return undefined;
      ordinary = fence(line);
    }
    return !block && !ordinary && cases.length ? cases : undefined;
  } catch { return undefined; }
}
