import { clearMocks, mockIPC } from '@tauri-apps/api/mocks';
import { cleanup, render, screen } from '@testing-library/react';
import { afterEach, expect, test, vi } from 'vitest';
import { Meet } from './Steps';

afterEach(() => { cleanup(); clearMocks(); vi.unstubAllGlobals(); });
test.each([[], ['web_fetch'], ['file_read', 'web_fetch', 'custom_tool']].map(tools => [tools]))('Meet lists the configured tools: %j', async tools => {
  vi.stubGlobal('isTauri', true);
  mockIPC(command => {
    if (command === 'setup_example') return { agent: `---\nname: web-reader\nkmx:\n  runtime: orka\n  schemaVersion: 1\n  tools: ${JSON.stringify(tools.map(name => ({ name, kind: 'builtin' })))}\n---\nRead a page.`, tests: 'A different test.' };
    throw Error(command);
  });
  render(<Meet onRead={() => {}} />);
  await screen.findByRole('button', { name: 'Read its instructions' });
  const rows = document.querySelectorAll('.workspace-tool code');
  expect([...rows].map(row => row.textContent)).toEqual(tools);
  expect(screen.queryByText(/checksum published/)).toBeNull();
});
test('Meet does not call an unreadable tool list empty', async () => {
  vi.stubGlobal('isTauri', true);
  mockIPC(() => ({ agent: '---\nname: web-reader\nkmx: broken\n---\nRead a page.', tests: 'A different test.' }));
  render(<Meet onRead={() => {}} />);
  expect(await screen.findByText('The tool list could not be read. Open its instructions to see the file.')).toBeTruthy();
  expect(screen.queryByText('No tools are configured.')).toBeNull();
});
