import { Disclosure } from '../components/Disclosure';
import { type ReactNode, type Ref, useEffect, useId, useLayoutEffect, useRef, useState } from 'react';
import type { CatalogTool, WorkspaceCatalogReply } from '../catalog';
import { Button, CloseIcon } from '../components/Button';
import { SAMPLE_DOCUMENT_ID } from './agent-api';
import { addTool, changeDescription, changeInstructions, changeModel, changeSoulInline, changeSoulReference, readForm, removeTool } from './agent-form';
import { builtInToolDescriptions, builtInTools, unsupportedLocalTool, unsupportedToolMessage, unsupportedToolReason } from './built-in-tools';
import FileLocation from './FileLocation';

function Unavailable({ label, reason, children, focusRef }: { label: string; reason: string; children: ReactNode; focusRef?: Ref<HTMLSpanElement> }) {
  return <span ref={focusRef} className="coming-control" tabIndex={0} role="group" aria-label={`${label}: ${reason}`} title={reason}>
    {children}<span className="control-tip" aria-hidden="true">{reason}</span>
  </span>;
}
function toolDescription(tool: CatalogTool): string {
  return tool.kind === 'builtin' && Object.hasOwn(builtInToolDescriptions, tool.name) ? builtInToolDescriptions[tool.name] : tool.description;
}
export default function AgentParts({ source, path, documentId = SAMPLE_DOCUMENT_ID, readOnly, setupPending = false, catalog, catalogLoading = false, remote = false, onPendingEdit, onChange }: {
  source: string; path?: string; documentId?: string; readOnly: boolean; setupPending?: boolean; catalog?: WorkspaceCatalogReply; catalogLoading?: boolean; remote?: boolean; onPendingEdit?: (pending: boolean) => void; onChange: (source: string) => void;
}) {
  const form = readForm(source); const id = useId();
  // Markdown's separating blank lines stay in the file, not above the prompt.
  const [separator, setSeparator] = useState(() => form.instructions.match(/^(?:[ \t]*\r?\n)+/)?.[0] ?? '');
  // A raw File edit can remove the original separator; do not hide a later user newline.
  if (separator && !form.instructions.startsWith(separator)) setSeparator('');
  const leading = form.instructions.startsWith(separator) ? separator : '';
  const instructions = form.instructions.slice(leading.length);
  const input = useRef<HTMLTextAreaElement>(null);
  const popover = useRef<HTMLDivElement>(null);
  useLayoutEffect(() => {
    const field = input.current;
    if (!field) return;
    const resize = () => { field.style.height = 'auto'; field.style.height = `${field.scrollHeight}px`; };
    resize();
    let mounted = true;
    void document.fonts?.ready.then(() => { if (mounted) resize(); });
    const observer = typeof ResizeObserver === 'undefined' ? undefined : new ResizeObserver(resize);
    observer?.observe(field);
    return () => { mounted = false; observer?.disconnect(); };
  }, [instructions]);
  const [picker, setPicker] = useState(false); const [query, setQuery] = useState('');
  const [soulDraft, setSoulDraft] = useState<{ source: string; value: string }>();
  const [soulError, setSoulError] = useState<string>();
  const editVersion = useRef(0); const pendingSoul = useRef(false);
  useEffect(() => {
    setSoulDraft(undefined);
    return () => {
      editVersion.current++;
      if (pendingSoul.current) setSoulError('A newer edit replaced the pending personality change. Re-enter it if needed.');
      pendingSoul.current = false; onPendingEdit?.(false);
    };
  }, [source, readOnly, onPendingEdit]);
  const add = useRef<HTMLButtonElement>(null);
  const addPlaceholder = useRef<HTMLSpanElement>(null);
  const focusAdd = () => (add.current ?? addPlaceholder.current)?.focus();
  const change = (value: string) => { if (!readOnly && value !== source) onChange(value); };
  const close = () => { setPicker(false); setQuery(''); focusAdd(); };
  const tools = catalog?.tools.state === 'ready' ? catalog.tools.items : undefined;
  const builtins = new Set(tools?.filter(tool => tool.kind === 'builtin').map(tool => tool.name));
  const choices = tools?.filter(tool => !form.tools.some(selected => selected.name === tool.name)
    && !(tool.kind === 'tool' && builtins.has(tool.name))
    && `${tool.name} ${toolDescription(tool)} ${tool.description} ${tool.kind === 'builtin' ? builtInTools[tool.name] ?? '' : ''}`.toLowerCase().includes(query.toLowerCase()));
  const models = catalog?.models.state === 'ready' ? catalog.models.items.filter(model => model.pickerEnabled !== false && model.chatSupported !== false) : undefined;
  const listed = models?.some(model => model.id === form.model);
  const knownModel = catalog?.models.state === 'ready' && catalog.models.items.some(model => model.id === form.model);
  const unlistedLabel = !remote && models ? knownModel ? ' (not offered for chat)' : ' (unlisted)' : '';
  const toolReason = readOnly ? 'You can change tools after setup.' : !form.toolsEditable ? 'Edit this agent’s tools in File view.'
    : setupPending ? 'Tools will be available when the local cluster is ready.' : catalogLoading ? 'Loading tools…' : tools === undefined ? 'Tools could not be read from this cluster. Check Clusters or edit File view.' : undefined;
  useEffect(() => {
    if (toolReason) { setPicker(false); setQuery(''); return; }
    if (picker) { popover.current?.showPopover?.(); popover.current?.querySelector('input')?.focus(); }
  }, [picker, toolReason]);
  const modelReason = readOnly ? 'You can change the model after setup.' : !form.modelEditable ? 'Edit the model in File view for this file’s format.'
    : setupPending ? 'Models will be available when the local cluster is ready.' : catalogLoading ? 'Loading models…' : models === undefined ? 'Models could not be read from this cluster. Check Clusters or edit File view.' : undefined;
  const labelFor = (name: string) => Object.hasOwn(builtInTools, name) ? builtInTools[name] : name;
  const editSoul = async (value: string) => {
    if (readOnly || !form.soulEditable) return;
    const version = ++editVersion.current;
    setSoulDraft({ source, value }); setSoulError(undefined); pendingSoul.current = true; onPendingEdit?.(true);
    try {
      const edited = await changeSoulInline(source, value);
      if (version === editVersion.current) { pendingSoul.current = false; change(edited); }
    } catch { if (version === editVersion.current) setSoulError('Personality could not be updated. The file is unchanged; try editing in File view.'); }
    // A failed conversion still has an unsaved visible draft. Run/Chat must not
    // silently use the earlier personality while that draft needs attention.
    finally { if (version === editVersion.current) onPendingEdit?.(pendingSoul.current); }
  };
  return <div className="agent-parts">
    {catalogLoading && <p className="muted" role="status">Loading available tools and models…</p>}
    {!form.metadataEditable && <p className="muted">This file’s settings can be edited in File view.</p>}
    <div className="agent-part">
      <div className="instructions-heading"><label htmlFor={`${id}-instructions`}>Instructions</label><p className="muted" id={`${id}-hint`}>The task the agent should do. Use Personality for its tone and style.</p></div>
      <textarea ref={input} id={`${id}-instructions`} className="field instructions-editor" aria-describedby={`${id}-hint`} value={instructions}
        readOnly={readOnly || !form.instructionsEditable} onChange={event => change(changeInstructions(source, leading + event.target.value))} />
      {!form.instructionsEditable && <p className="muted">Finish the file’s opening settings in File view to edit its instructions here.</p>}
    </div>
    <section className="agent-part" aria-labelledby={`${id}-tools`}>
      <h3 id={`${id}-tools`}>It can use</h3>
      <div className="agent-tool-row">
        {form.tools.map(tool => {
          const name = tool.name ?? 'Unrecognised tool entry'; const label = tool.builtin ? labelFor(name) : name;
          const observation = tools?.find(item => item.kind === tool.kind && item.name === name);
          return <span className="tool-pill chip" key={tool.index} title={observation?.description}>
            <span>{label}</span>{label !== name && <code>{name}</code>}
            {tool.kind === 'tool' && <span className="muted">(Cluster tool{builtins.has(name) ? ', built-in takes precedence' : ''})</span>}
            {!readOnly && form.toolsEditable && ['builtin', 'tool'].includes(tool.kind ?? '') && <Button kind="icon" aria-label={`Remove ${label}`} onClick={() => { change(removeTool(source, tool.index)); focusAdd(); }}><CloseIcon /></Button>}
          </span>;
        })}
        {toolReason ? <Unavailable label="Add a tool" reason={toolReason} focusRef={addPlaceholder}><Button kind="secondary" type="button" disabled title={toolReason}>Add a tool</Button></Unavailable>
          : <Button kind="secondary" type="button" ref={add} aria-expanded={picker} aria-controls={`${id}-picker`} onClick={() => { if (picker) close(); else setPicker(true); }}><span aria-hidden="true">+ </span>Add a tool</Button>}
      </div>
      {!remote && form.tools.some(tool => tool.name && unsupportedLocalTool(tool.name)) && <p role="alert">{unsupportedToolMessage}</p>}
      {toolReason && !catalogLoading && <p className="muted">{toolReason}</p>}
      {picker && !toolReason && <div role="dialog" aria-label="Choose a tool" ref={popover} popover="auto" id={`${id}-picker`} className="tool-picker" onToggle={event => { if (event.newState === 'closed') { setPicker(false); setQuery(''); } }} onKeyDown={event => { if (event.key === 'Escape') { event.preventDefault(); close(); } }}>
        <Button kind="icon" className="picker-close" aria-label="Close picker" title="Close picker" onClick={close}><CloseIcon /></Button>
        <label htmlFor={`${id}-search`}>Find a tool</label>
        <input className="field" id={`${id}-search`} type="search" autoFocus value={query} onChange={event => setQuery(event.target.value)} />
        <p className="muted">Choose a tool to add to your agent.</p>
        <ul aria-label="Cluster tools">{choices?.map((tool, index) => {
          const blocked = !remote && unsupportedLocalTool(tool.name);
          const label = tool.kind === 'builtin' ? labelFor(tool.name) : tool.name;
          const reasonId = `${id}-tool-reason-${index}`;
          return <li key={`${tool.kind}:${tool.name}`}><div role={blocked ? 'group' : undefined} tabIndex={blocked ? 0 : undefined} aria-label={blocked ? label : undefined} aria-describedby={blocked ? reasonId : undefined}>
            <button type="button" disabled={blocked} onClick={() => { change(addTool(source, tool.name, tool.kind)); close(); }}>
              <span className="tool-option-heading"><span>{label}</span><span className="muted">{tool.kind === 'builtin' ? 'Built-in' : 'Cluster tool'}</span></span>
              {tool.kind === 'builtin' && labelFor(tool.name) !== tool.name && <code>{tool.name}</code>}
              <span className="tool-description">{toolDescription(tool)}</span>
              {tool.available === false && <span className="muted">Observed unavailable</span>}
            </button>
            {blocked && <p className="tool-unavailable-reason" id={reasonId}>{unsupportedToolReason}</p>}
          </div></li>;
        })}</ul>
        {!choices?.length && <p className="muted">{tools?.length ? 'No matching tools to add.' : 'No tools listed by this cluster.'}</p>}
      </div>}
    </section>
    <div className="agent-part">
      <label htmlFor={`${id}-model`}>It thinks with</label>
      <div className="agent-tool-row">
        <select id={`${id}-model`} className="field model-select" disabled={!!modelReason} title={modelReason} value={form.model ?? ''} onChange={event => change(changeModel(source, event.target.value))}>
          {!listed && <option value={form.model ?? ''}>{form.model ? `${form.model}${unlistedLabel}` : 'No model in this file'}</option>}
          {models?.map(model => <option key={model.id} value={model.id}>{model.id}</option>)}
        </select>
        <span className="muted">{catalogLoading ? 'The model saved in your file is shown while choices load.' : modelReason ?? (remote ? models?.length ? 'The selected AI connection’s configured default. Other models can be entered in File view.' : 'No default model is configured for this AI connection. Enter a model in File view.' : models?.length ? `Default model: ${catalog?.defaultModel}` : 'No models listed by the local cluster.')}</span>
      </div>
      {!remote && !listed && models && form.model && <p className="muted">{knownModel ? 'This model is not offered for chat by the connected AI. Choose another model to use it here.' : 'This model is saved in your file but is not listed by the connected AI. It may not run here. Choose a listed model or check your AI connection.'}</p>}
    </div>
    <Disclosure className="agent-settings" label="More settings">
      <div className="agent-part">
        <label htmlFor={`${id}-description`}>Description</label>
        <textarea id={`${id}-description`} placeholder="A short summary to help you recognise this agent." className="field description-editor" readOnly={readOnly || !form.descriptionEditable} value={form.description ?? ''}
          onChange={event => change(changeDescription(source, event.target.value))} />
        {!form.descriptionEditable && <p className="muted">Edit the description in File view for this file’s format.</p>}
      </div>
      <div className="agent-part">
        {form.soul?.configMapRef ? <>
          <h3>Shared personality</h3>
          <p className="muted">Tone and style, separate from the task in Instructions. This file refers to personality text stored on the cluster. Bridge cannot show that text here.</p>
          <Disclosure label="Shared text settings"><p className="muted">The ConfigMap supplies the text at run time. The digest checks that it is the expected version.</p>
          {(['name', 'key', 'digest'] as const).map(field => <label className="soul-reference-field" key={field}>
            {field === 'name' ? 'ConfigMap name' : field === 'key' ? 'ConfigMap key' : 'Expected digest'}
            <input className="field" readOnly={readOnly || !form.soulEditable} value={field === 'digest' ? form.soul?.digest ?? '' : form.soul!.configMapRef![field]}
              onChange={event => change(changeSoulReference(source, field, event.target.value))} />
          </label>)}</Disclosure>
        </> : <>
          <label htmlFor={`${id}-soul`}>Personality</label>
          <p className="muted" id={`${id}-soul-hint`}>Tone and style to keep across tasks. Put the task itself in Instructions.</p>
          <textarea id={`${id}-soul`} placeholder="For example: Be concise, use plain words and explain unfamiliar terms." aria-describedby={`${id}-soul-hint`} className="field description-editor" readOnly={readOnly || !form.soulEditable} value={soulDraft?.source === source ? soulDraft.value : form.soul?.inline ?? ''}
            onChange={event => { void editSoul(event.target.value); }} />
          {form.soul?.digest && <p className="muted">The existing digest is updated with each personality edit.</p>}
          {!form.soulEditable && <p className="muted">Edit personality in File view for this file’s format.</p>}
          {soulError && <p role="alert">{soulError}</p>}
        </>}
      </div>
      <div className="agent-part"><h3>File</h3><FileLocation path={path} documentId={documentId} /></div>
    </Disclosure>
  </div>;
}
