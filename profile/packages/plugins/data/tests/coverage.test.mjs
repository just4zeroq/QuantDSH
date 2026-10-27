import test from 'node:test';
import assert from 'node:assert/strict';
import { createRequire } from 'node:module';
import { fileURLToPath } from 'node:url';

const require = createRequire(new URL('../../../../apps/web/package.json', import.meta.url));
const { build } = require('esbuild');
const built = await build({
  stdin: { contents: "export * from './coverage';", resolveDir: fileURLToPath(new URL('../src/domain/', import.meta.url)), loader: 'ts' },
  bundle: true, platform: 'node', format: 'esm', write: false,
});
const { expectedBars, detectGaps, coverageMatrix } = await import(`data:text/javascript;base64,${Buffer.from(built.outputFiles[0].text).toString('base64')}`);

const MIN = 60_000;

test('expectedBars snaps to interval grid', () => {
  // 1m bars from 00:00:30 to 00:02:30 -> 3 full minutes 00:00,00:01,00:02
  const bars = expectedBars(Date.UTC(2026, 9, 26, 0, 0, 30), Date.UTC(2026, 9, 26, 0, 2, 30), MIN);
  assert.deepEqual(bars, [
    Date.UTC(2026, 9, 26, 0, 0, 0),
    Date.UTC(2026, 9, 26, 0, 1, 0),
    Date.UTC(2026, 9, 26, 0, 2, 0),
  ]);
});

test('detectGaps reports missing without inventing data', () => {
  const start = Date.UTC(2026, 9, 26, 0, 0, 0);
  const end = Date.UTC(2026, 9, 26, 0, 4, 0); // 5 expected minutes
  const actual = [start, start + MIN, start + 3 * MIN, start + 4 * MIN]; // missing start+2min
  const report = detectGaps(start, end, MIN, actual);
  assert.equal(report.expected, 5);
  assert.equal(report.present, 4);
  assert.deepEqual(report.missing, [start + 2 * MIN]);
  assert.equal(report.coverageRatio, 0.8);
});

test('full coverage has no missing bars', () => {
  const start = Date.UTC(2026, 9, 26, 0, 0, 0);
  const end = Date.UTC(2026, 9, 26, 0, 9, 0);
  const actual = expectedBars(start, end, MIN);
  const report = detectGaps(start, end, MIN, actual);
  assert.equal(report.missing.length, 0);
  assert.equal(report.coverageRatio, 1);
});

test('coverageMatrix leaves missing hours at present=0', () => {
  // 1m interval, 1 hour on 2026-10-26 has 60 bars. Supply only the first 10.
  const year = 2026, month = 10;
  const actual = Array.from({ length: 10 }, (_, i) => Date.UTC(year, month - 1, 26, 0, i, 0));
  const cells = coverageMatrix(year, month, MIN, actual);
  assert.equal(cells.length, 24 * 31); // Oct has 31 days
  const firstHour = cells.find((c) => c.day === '2026-10-26' && c.hour === 0);
  assert.equal(firstHour.present, 10);
  assert.equal(firstHour.expected, 60);
  const emptyHour = cells.find((c) => c.day === '2026-10-26' && c.hour === 5);
  assert.equal(emptyHour.present, 0); // not skipped — explicitly empty
});
