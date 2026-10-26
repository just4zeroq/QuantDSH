import assert from 'node:assert/strict';
import { test } from 'node:test';
import { withoutRedundantAgentTeamProfile } from '../../scripts/preview-agent-team.mjs';

test('preview keeps only the QuantDSH-owned Agent Team bundle', () => {
  const input = { dsh: { profile: { bundles: ['@deepseek-ai/dsh-base', 'quantdsh-plugin-experts', '@deepseek-ai/dsh-experimental-agent-team-profile', 'quantdsh-plugin-library'] } } };
  const output = withoutRedundantAgentTeamProfile(input);
  assert.deepEqual(output.dsh.profile.bundles, ['@deepseek-ai/dsh-base', 'quantdsh-plugin-experts', 'quantdsh-plugin-library']);
  assert.equal(input.dsh.profile.bundles.length, 4);
  assert.equal(withoutRedundantAgentTeamProfile(output), undefined);
});

test('preview leaves a standalone upstream Team profile alone without QuantDSH experts', () => {
  const input = { dsh: { profile: { bundles: ['@deepseek-ai/dsh-experimental-agent-team-profile'] } } };
  assert.equal(withoutRedundantAgentTeamProfile(input), undefined);
});
