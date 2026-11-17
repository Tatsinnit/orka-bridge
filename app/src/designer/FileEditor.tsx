import { useLayoutEffect, useRef } from 'react';

// Decoration only: unfinished YAML and unknown fields remain editable verbatim.
export default function FileEditor({ value, onChange, onSave }: { value: string; onChange: (value: string) => void; onSave: () => void }) {
  const input = useRef<HTMLTextAreaElement>(null);
  useLayoutEffect(() => {
    const field = input.current;
    if (!field) return;
    const resize = () => {
      field.style.height = 'auto';
      field.style.height = `${field.scrollHeight + field.offsetHeight - field.clientHeight}px`;
    };
    resize(); let mounted = true;
    void document.fonts?.ready.then(() => { if (mounted) resize(); });
    const observer = typeof ResizeObserver === 'undefined' ? undefined : new ResizeObserver(resize);
    observer?.observe(field);
    return () => { mounted = false; observer?.disconnect(); };
  }, [value]);
  // Decoration must not turn a supported 1 MiB document into a million DOM nodes.
  const lines = value.length <= 100_000 ? value.replace(/\r\n?/g, '\n').split('\n') : undefined;
  const highlighted = lines && lines.length <= 2000 ? lines : undefined;
  let settings = false;
  return <>{!highlighted && <p className="muted">Syntax colouring and line numbers are off for this large file. Editing and saving still work.</p>}<div className="file-editor">
    {highlighted && <pre className="file-highlight" aria-hidden="true">{highlighted.map((line, index) => {
      const delimiter = line === '---' && (index === 0 || settings);
      if (delimiter) settings = !settings;
      const key = settings && !delimiter ? line.match(/^(\s*(?:-\s+)?)([\w.-]+)(:)/) : null;
      return <span className="file-line" data-line={index + 1} key={index}>{delimiter ? <span className="file-delimiter">{line}</span>
        : key ? <>{key[1]}<span className="file-key">{key[2]}</span>{line.slice(key[1].length + key[2].length)}</>
          : <span className={/^\s*#/.test(line) ? 'file-comment' : undefined}>{line || '\n'}</span>}</span>;
    })}</pre>}
    <label className="sr-only" htmlFor="agent-file">Agent file</label>
    <textarea ref={input} id="agent-file" aria-describedby="file-hint" className={`agent-editor${highlighted ? '' : ' file-plain'}`} spellCheck={false} autoCapitalize="off" autoCorrect="off" value={value}
      onChange={event => onChange(event.target.value)} onBlur={onSave} onKeyDown={event => {
        if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === 's') { event.preventDefault(); onSave(); }
      }} />
  </div></>;
}
