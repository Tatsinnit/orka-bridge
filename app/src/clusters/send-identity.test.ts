import { describe, expect, it } from 'vitest';
import { isSummary } from './send-api';

const legacy = {
  id: '00000000-0000-4000-8000-000000000001', source: { agent: 'a'.repeat(64), tests: 'b'.repeat(64) },
  targetDisplay: 'team · agents', targetUids: { serviceNamespace: 'control', serviceUid: 'service', deploymentNamespace: 'control', deploymentUid: 'deployment', watchedNamespace: 'agents' },
  provider: 'model', plannedAgentName: 'agent', agentUid: 'uid', verdict: 'unknown', time: 1,
};
describe('saved destination identity', () => {
  it('keeps old summaries readable and accepts verified destination hashes', () => {
    expect(isSummary(legacy)).toBe(true);
    expect(isSummary({ ...legacy, targetUids: { ...legacy.targetUids, clusterIdentity: 'a'.repeat(64) } })).toBe(true);
  });
  it.each([null, '', 'not-a-hash', 'A'.repeat(64), 'a'.repeat(65)])('rejects malformed present identity %s', clusterIdentity => {
    expect(isSummary({ ...legacy, targetUids: { ...legacy.targetUids, clusterIdentity } })).toBe(false);
  });
});
