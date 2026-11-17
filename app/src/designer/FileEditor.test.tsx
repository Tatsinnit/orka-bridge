import { cleanup, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, expect, test, vi } from 'vitest';
import FileEditor from './FileEditor';
afterEach(cleanup);
test('large supported files stay editable without building a node for every line', () => {
  const source = '\n'.repeat(5000); const changed = vi.fn();
  render(<FileEditor value={source} onChange={changed} onSave={() => {}} />);
  expect(document.querySelector('.file-highlight')).toBeNull();
  const field = screen.getByRole('textbox') as HTMLTextAreaElement;
  expect(field.value).toBe(source); expect(field.classList.contains('file-plain')).toBe(true);
  fireEvent.change(field, { target: { value: source + 'Kept' } });
  expect(changed).toHaveBeenLastCalledWith(source + 'Kept');
});
test('lone carriage returns use the same lines as the native textarea', () => {
  render(<FileEditor value={'one\rtwo\r'} onChange={() => {}} onSave={() => {}} />);
  expect((screen.getByRole('textbox') as HTMLTextAreaElement).value).toBe('one\ntwo\n');
  expect(document.querySelectorAll('.file-line')).toHaveLength(3);
});
test('highlights settings and numbers source lines without parsing or rewriting unfinished files', () => {
  const source = '---\r\nname: reader\r\nkmx:\r\n  tools: [\r\n---\r\n\r\n# Instructions\r\n<img src=x onerror=alert(1)>\r\n';
  const changed = vi.fn(); const saved = vi.fn();
  const view = render(<FileEditor value={source} onChange={changed} onSave={saved} />);
  const field = screen.getByRole('textbox', { name: 'Agent file' }) as HTMLTextAreaElement;
  expect(field.value).toBe(source.replaceAll('\r\n', '\n'));
  expect(document.querySelectorAll('.file-key').length).toBe(3);
  expect(document.querySelectorAll('.file-line')).toHaveLength(9);
  expect(document.querySelector('.file-highlight img')).toBeNull();
  expect(document.querySelector('.file-highlight')?.textContent).toContain('<img src=x onerror=alert(1)>');
  fireEvent.change(field, { target: { value: '---\nname: [\n\nDraft\n' } });
  expect(changed).toHaveBeenLastCalledWith('---\nname: [\n\nDraft\n');
  fireEvent.keyDown(field, { key: 's', ctrlKey: true }); expect(saved).toHaveBeenCalledTimes(1);
  fireEvent.blur(field); expect(saved).toHaveBeenCalledTimes(2);
  view.rerender(<FileEditor value={'---\nname: [\n\nDraft\n'} onChange={changed} onSave={saved} />);
  expect(field.value).toBe('---\nname: [\n\nDraft\n');
  expect(document.querySelectorAll('.file-line')).toHaveLength(5);
});
