import { readFileSync } from 'node:fs';
import { parse } from 'yaml';
import { describe, expect, test } from 'vitest';

type Step = { id?: string; name?: string; uses?: string; run?: string; if?: string; with?: Record<string, string> };
type Workflow = {
  on: Record<string, unknown>;
  concurrency: { group: string; 'cancel-in-progress': string };
  jobs: Record<string, { if?: string; uses?: string; 'timeout-minutes'?: string; strategy?: { matrix?: { include?: Record<string, unknown>[] } }; steps?: Step[] }>;
};

const workflow = parse(readFileSync('../.github/workflows/checks.yml', 'utf8')) as Workflow;
const allSteps = Object.values(workflow.jobs).flatMap(job => job.steps ?? []);
const nativeSteps = workflow.jobs.native.steps ?? [];
const rustPaths = ['~/.cargo/registry', '~/.cargo/git', 'app/src-tauri/target'];
const parsedPaths = (step: Step) => step.with?.path.trimEnd().split('\n');
const stepIndex = (name: string) => {
  const index = nativeSteps.findIndex(step => step.name === name);
  expect(index, `${name} must remain in the native job`).toBeGreaterThanOrEqual(0);
  return index;
};

describe('App workflow cache ownership', () => {
  test('runs only for pushes and pull requests while preserving main publishers', () => {
    expect(Object.keys(workflow.on).sort()).toEqual(['pull_request', 'push']);
    expect(workflow.on.push).toBeNull();
    expect(workflow.on.pull_request).toBeNull();
    expect(workflow.concurrency).toEqual({
      group: 'app-${{ github.workflow }}-${{ github.ref }}',
      'cancel-in-progress': "${{ github.ref != 'refs/heads/main' }}",
    });
  });

  test('restores the rotated shared Rust namespace on every ref', () => {
    expect(workflow.jobs.native.if).toBeUndefined();
    expect(workflow.jobs.native.uses).toBeUndefined();
    const restore = nativeSteps.filter(step => step.uses?.toLowerCase().startsWith('actions/cache/restore@'));
    expect(restore).toHaveLength(1);
    expect(restore[0].id).toBe('rust-cache');
    expect(restore[0].if).toBeUndefined();
    expect(parsedPaths(restore[0])).toEqual(rustPaths);
    expect(restore[0].with?.key).toBe("${{ matrix.rust-cache }}-shared-v2-${{ hashFiles('app/src-tauri/Cargo.lock', 'app/src-tauri/Cargo.toml') }}");
    expect(restore[0].with?.['restore-keys'].trimEnd().split('\n')).toEqual([
      '${{ matrix.rust-cache }}-shared-v2-',
      '${{ matrix.rust-cache-legacy || matrix.rust-cache }}-shared-v2-',
      '${{ matrix.rust-cache-restore || matrix.rust-cache }}-shared-v2-',
    ]);
  });

  test('extends only the demonstrated cold release and bundle budgets', () => {
    expect(workflow.jobs.native['timeout-minutes']).toBe('${{ matrix.timeout || 30 }}');
    const rows = workflow.jobs.native.strategy?.matrix?.include ?? [];
    const overrides = rows.filter(row => row.timeout !== undefined);
    expect(overrides).toEqual([
      expect.objectContaining({
        os: 'windows-2022',
        artifact: 'windows-release-checks',
        suite: 'release-checks',
        timeout: 45,
      }),
      expect.objectContaining({
        os: 'macos-15',
        artifact: 'macos-arm64-light',
        suite: 'bundle',
        theme: 'light',
        timeout: 45,
      }),
      expect.objectContaining({
        os: 'macos-15',
        artifact: 'macos-arm64-dark',
        suite: 'bundle',
        theme: 'dark',
        timeout: 45,
      }),
    ]);
  });

  test('publishes successful Rust output only from main', () => {
    const saves = nativeSteps.filter(step => step.uses?.toLowerCase().startsWith('actions/cache/save@'));
    expect(saves.map(step => step.name).sort()).toEqual([
      'Preserve native bundle cache',
      'Preserve verified Rust cache',
    ]);
    expect(saves.map(step => step.if)).toEqual([
      "github.event_name == 'push' && github.ref == 'refs/heads/main' && (matrix.suite == 'debug-checks' || matrix.suite == 'release-checks') && steps.rust-cache.outputs.cache-hit != 'true'",
      "github.event_name == 'push' && github.ref == 'refs/heads/main' && (matrix.suite == 'bundle' || matrix.suite == 'lifecycle-fixtures' || matrix.suite == 'model-fixtures') && steps.rust-cache.outputs.cache-hit != 'true'",
    ]);
    for (const save of saves) {
      expect(parsedPaths(save)).toEqual(rustPaths);
      expect(save.with?.key).toBe('${{ steps.rust-cache.outputs.cache-primary-key }}');
    }
    expect(nativeSteps.some(step => step.name?.includes('after a failed native check'))).toBe(false);
    expect(stepIndex('Preserve verified Rust cache')).toBeGreaterThan(stepIndex('Release Rust checks'));
    expect(stepIndex('Preserve verified Rust cache')).toBeGreaterThan(stepIndex('Debug Rust checks'));
    expect(stepIndex('Preserve native bundle cache')).toBeGreaterThan(stepIndex('Mac app bundle and native walk'));
    expect(stepIndex('Preserve native bundle cache')).toBeGreaterThan(stepIndex('Windows workspace upgrade renderer fixtures (no VM or credentials)'));
    const lastArtifact = nativeSteps.reduce((last, step, index) => step.uses?.toLowerCase().startsWith('actions/upload-artifact@') ? index : last, -1);
    expect(lastArtifact, 'native artifact uploads must remain in the job').toBeGreaterThanOrEqual(0);
    expect(stepIndex('Preserve native bundle cache')).toBeGreaterThan(lastArtifact);
  });

  test('uses only explicit read-only or main-only Rust cache actions', () => {
    expect(nativeSteps.some(step => step.uses?.startsWith('./'))).toBe(false);
    const cacheActions = allSteps.filter(step => step.uses?.toLowerCase().startsWith('actions/cache'));
    expect(cacheActions).toHaveLength(3);
    expect(cacheActions.every(step => step.uses?.toLowerCase() === 'actions/cache/restore@0057852bfaa89a56745cba8c7296529d2fc39830'
      || step.uses?.toLowerCase() === 'actions/cache/save@0057852bfaa89a56745cba8c7296529d2fc39830')).toBe(true);
    expect(workflow.jobs.frontend.steps?.some(step => step.run === 'npm test')).toBe(true);
  });
});
