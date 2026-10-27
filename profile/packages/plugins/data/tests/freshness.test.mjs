import test from 'node:test';
import assert from 'node:assert/strict';
import { createRequire } from 'node:module';
import { fileURLToPath } from 'node:url';

const require = createRequire(new URL('../../../../apps/web/package.json', import.meta.url));
const { build } = require('esbuild');
const built = await build({
  stdin: { contents: "export * from './freshness';", resolveDir: fileURLToPath(new URL('../src/domain/', import.meta.url)), loader: 'ts' },
  bundle: true, platform: 'node', format: 'esm', write: false,
});
const { gradeFreshness, freshnessPresentation } = await import(`data:text/javascript;base64,${Buffer.from(built.outputFiles[0].text).toString('base64')}`);

const MIN = 60_000, HOUR = 60 * MIN;
const now = Date.UTC(2026, 9, 26, 12, 0, 0); // fixed "now"

test('fresh within 1.5 intervals', () => {
  const f = gradeFreshness(now - 0.5 * MIN, now, MIN); // 1m bar, 30s lag
  assert.equal(f.level, 'fresh');
  assert.equal(freshnessPresentation(f).text, '实时');
  assert.equal(freshnessPresentation(f).tone, 'ok');
});

test('lagging between 1.5 and 3 intervals', () => {
  const f = gradeFreshness(now - 2 * MIN, now, MIN);
  assert.equal(f.level, 'lagging');
  assert.equal(freshnessPresentation(f).tone, 'warn');
});

test('stale within 24h, gapped beyond', () => {
  const stale = gradeFreshness(now - 3 * HOUR, now, MIN);
  assert.equal(stale.level, 'stale');
  const gapped = gradeFreshness(now - 48 * HOUR, now, MIN);
  assert.equal(gapped.level, 'gapped');
});

test('empty when no latest bar', () => {
  const f = gradeFreshness(null, now, MIN);
  assert.equal(f.level, 'empty');
  assert.equal(freshnessPresentation(f).text, '无数据');
});

test('lagBars counts bars, not wall time', () => {
  const f = gradeFreshness(now - 5 * MIN, now, MIN);
  assert.equal(f.lagBars, 5);
});

test('invalid input rejected', () => {
  assert.throws(() => gradeFreshness(123, now, -1));
  assert.throws(() => gradeFreshness(NaN, now, MIN));
});
