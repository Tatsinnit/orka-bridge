import { clearMocks, mockIPC } from '@tauri-apps/api/mocks';
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, expect, test, vi } from 'vitest';
import App from './App';

vi.mock('./designer/Example', () => ({ default: (props: { workspaceReady: boolean; setupPending: boolean }) => <output data-testid="designer-admission">{JSON.stringify(props)}</output> }));
afterEach(() => { cleanup(); clearMocks(); vi.unstubAllGlobals(); localStorage.clear(); });
const none = { state: 'none', transaction: null, createdAt: null, sourceReleases: [], targetRelease: null, retainedAllocatedBytes: null, additionalBytes: null, credential: 'absent', canCancel: false, busy: false, error: null, repeated: false, canReplace: false, canRestore: false };
test.each(['hung', 'failed'] as const)('unreadable replaced upgrade blocks local Run/Chat but not file editing or remote Send (%s)', async observation => {
  vi.stubGlobal('isTauri', true); localStorage.setItem('bridge.welcomeSeen.v1', '1');
  let known = false;
  mockIPC(command => {
    if (command === 'setup_status') return { selected: 'localVm', localVm: 'ready', model: 'available', workspaceReplaced: true };
    if (command === 'workspace_upgrade_status') {
      if (known) return none;
      if (observation === 'failed') throw Error('Lost upgrade status');
      return new Promise(() => {});
    }
    if (command === 'setup_requirements') return null;
    throw Error('Unexpected ' + command);
  });
  render(<App />);
  await screen.findByRole('region', { name: 'This computer' });
  fireEvent.click(screen.getByRole('button', { name: 'Designer' }));
  const admission = await screen.findByTestId('designer-admission');
  await waitFor(() => expect(JSON.parse(admission.textContent!).workspaceReady).toBe(false));
  expect(JSON.parse(admission.textContent!).setupPending).toBe(false);
  known = true; fireEvent(window, new Event('focus'));
  await waitFor(() => expect(JSON.parse(admission.textContent!).workspaceReady).toBe(true));
});
