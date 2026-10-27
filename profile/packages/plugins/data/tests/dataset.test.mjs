import test from 'node:test';
import assert from 'node:assert/strict';
import { createRequire } from 'node:module';
import { fileURLToPath } from 'node:url';

const require = createRequire(new URL('../../../../apps/web/package.json', import.meta.url));
const { build } = require('esbuild');
const built = await build({
  // market.ts 必须一起导出：Market / Interval / INTERVAL_MS 已统一到它里面，
  // dataset.ts 只是引用而不再转出（否则 domain/index.ts 的 export * 会重名冲突）。
  stdin: { contents: "export * from './dataset'; export * from './market'; export * from './symbol'; export * from './freshness'; export * from './coverage'; export * from './contract';", resolveDir: fileURLToPath(new URL('../src/domain/', import.meta.url)), loader: 'ts' },
  bundle: true, platform: 'node', format: 'esm', write: false,
});
const { summarizeDataset, INTERVAL_MS } = await import(`data:text/javascript;base64,${Buffer.from(built.outputFiles[0].text).toString('base64')}`);

const now = Date.UTC(2026, 9, 26, 12, 0, 0);
const base = {
  id: 'binance:BTCUSDT:1m',
  market: 'crypto',
  exchange: 'binance',
  symbol: 'binance:BTCUSDT',
  interval: '1m',
  startTs: Date.UTC(2026, 9, 26, 0, 0, 0),
  endTs: Date.UTC(2026, 9, 26, 0, 9, 0),
  contract: { asOf: Date.UTC(2026, 9, 26), ingestedAt: now, closedOnly: true },
};

test('summarize combines freshness, coverage and contract', () => {
  const record = {
    ...base,
    latestBarTs: now - 30_000,
    presentBars: [
      Date.UTC(2026, 9, 26, 0, 0, 0),
      Date.UTC(2026, 9, 26, 0, 1, 0),
      Date.UTC(2026, 9, 26, 0, 3, 0),
      Date.UTC(2026, 9, 26, 0, 4, 0),
      Date.UTC(2026, 9, 26, 0, 5, 0),
      Date.UTC(2026, 9, 26, 0, 6, 0),
      Date.UTC(2026, 9, 26, 0, 7, 0),
      Date.UTC(2026, 9, 26, 0, 8, 0),
      Date.UTC(2026, 9, 26, 0, 9, 0),
    ],
  };
  const s = summarizeDataset(record, now);
  assert.equal(s.freshness.level, 'fresh');
  assert.equal(s.coverage.expected, 10);
  assert.equal(s.coverage.present, 9); // missing minute 2
  assert.equal(s.coverage.missing.length, 1);
  assert.equal(s.contract.valid, true);
});

test('missing contract fails the summary contract check', () => {
  const record = { ...base, latestBarTs: now - 30_000, presentBars: [], contract: { asOf: null, ingestedAt: null, closedOnly: false } };
  const s = summarizeDataset(record, now);
  assert.equal(s.contract.valid, false);
  assert.equal(s.freshness.level, 'fresh'); // freshness independent of contract
});

test('interval map is aligned to real durations', () => {
  assert.equal(INTERVAL_MS['1m'], 60_000);
  assert.equal(INTERVAL_MS['1d'], 86_400_000);
});
