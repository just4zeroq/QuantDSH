import test from 'node:test';
import assert from 'node:assert/strict';
import { createRequire } from 'node:module';
import { fileURLToPath } from 'node:url';

const require = createRequire(new URL('../../../../apps/web/package.json', import.meta.url));
const { build } = require('esbuild');
const built = await build({
  stdin: { contents: "export * from './contract';", resolveDir: fileURLToPath(new URL('../src/domain/', import.meta.url)), loader: 'ts' },
  bundle: true, platform: 'node', format: 'esm', write: false,
});
const { validateContract } = await import(`data:text/javascript;base64,${Buffer.from(built.outputFiles[0].text).toString('base64')}`);

const valid = { asOf: Date.UTC(2026, 9, 1), ingestedAt: Date.UTC(2026, 9, 26), closedOnly: true };

test('valid contract passes', () => {
  const c = validateContract(valid);
  assert.equal(c.valid, true);
  assert.equal(c.missing.length, 0);
  assert.equal(c.reason, null);
});

test('missing all fields fails closed', () => {
  const c = validateContract(null);
  assert.equal(c.valid, false);
  assert.deepEqual([...c.missing].sort(), ['asOf', 'closedOnly', 'ingestedAt']);
});

test('partial missing lists only the absent keys', () => {
  const c = validateContract({ asOf: 123, closedOnly: true }); // ingestedAt missing
  assert.equal(c.valid, false);
  assert.deepEqual(c.missing, ['ingestedAt']);
});

test('wrong types are treated as missing', () => {
  const c = validateContract({ asOf: '2026', ingestedAt: 1, closedOnly: 'yes' });
  assert.equal(c.valid, false);
  assert.deepEqual([...c.missing].sort(), ['asOf', 'closedOnly']);
});

test('closedOnly=false is still valid (boolean, not truthyness)', () => {
  const c = validateContract({ asOf: 1, ingestedAt: 2, closedOnly: false });
  assert.equal(c.valid, true);
});
