import test from 'node:test';
import assert from 'node:assert/strict';
import { loadSrc } from './_load.mjs';

const { MarketStore } = await loadSrc('engine/market');

const DAY = 86_400_000;
const MIN = 60_000;
const T0 = Date.UTC(2026, 9, 26, 0, 0, 0); // 2026-10-26T00:00:00Z
const PROV = { source: 'test' };

function bar(ts, close = 100, symbol = 'BTCUSDT') {
  return {
    symbol,
    interval: '1m',
    ts,
    open: close - 1,
    high: close + 1,
    low: close - 2,
    close,
    volume: 1000,
    turnover: close * 1000,
    closed: true,
  };
}

async function freshStore() {
  const s = await MarketStore.open(':memory:');
  return s;
}

test('upsert is idempotent — writing the same bar twice leaves one row', async () => {
  const s = await freshStore();
  await s.upsertBars([bar(T0, 100)], PROV);
  await s.upsertBars([bar(T0, 111)], PROV); // 重复写入：应覆盖而非新增
  const rows = await s.queryBars({ symbol: 'BTCUSDT', interval: '1m' });
  assert.equal(rows.length, 1, '重复写入不应产生第二行');
  assert.equal(rows[0].close, 111, '后写入的值应覆盖先前的');
  s.close();
});

test('batch upsert writes all rows and query respects range', async () => {
  const s = await freshStore();
  const bars = Array.from({ length: 120 }, (_, i) => bar(T0 + i * MIN, 100 + i));
  await s.upsertBars(bars, PROV);
  assert.equal(await s.countBars(), 120);

  const first10 = await s.queryBars({ symbol: 'BTCUSDT', interval: '1m', from: T0, to: T0 + 9 * MIN });
  assert.equal(first10.length, 10);
  assert.equal(first10[0].ts, T0);
  // ts 必须是 number，不能是 BigInt（BigInt 会让 JSON 序列化直接抛错）
  assert.equal(typeof first10[0].ts, 'number');
  s.close();
});

test('correctBar patches a single bar (the update path Parquet cannot do)', async () => {
  const s = await freshStore();
  await s.upsertBars([bar(T0, 100)], PROV);
  const r = await s.correctBar('BTCUSDT', '1m', T0, { close: 999, high: 1000 });
  assert.equal(r.changed, true);
  const rows = await s.queryBars({ symbol: 'BTCUSDT', interval: '1m' });
  assert.equal(rows[0].close, 999);
  assert.equal(rows[0].high, 1000);
  assert.equal(rows[0].low, 98, '未 patch 的字段应保持原值');
  assert.equal(await s.countBars(), 1, '修正不应新增行');
  s.close();
});

test('deleteRange removes an interval for re-pull', async () => {
  const s = await freshStore();
  await s.upsertBars(Array.from({ length: 10 }, (_, i) => bar(T0 + i * MIN)), PROV);
  const res = await s.deleteRange('BTCUSDT', '1m', T0, T0 + 4 * MIN);
  assert.equal(res.deleted, 5);
  assert.equal(await s.countBars(), 5);
  s.close();
});

test('rangeOf reports min/max/count', async () => {
  const s = await freshStore();
  await s.upsertBars([bar(T0), bar(T0 + 5 * MIN), bar(T0 + 9 * MIN)], PROV);
  const r = await s.rangeOf('BTCUSDT', '1m');
  assert.equal(r.minTs, T0);
  assert.equal(r.maxTs, T0 + 9 * MIN);
  assert.equal(r.count, 3);
  s.close();
});

test('timestamps returns numeric ms array for coverage/gap detection', async () => {
  const s = await freshStore();
  await s.upsertBars([bar(T0), bar(T0 + MIN), bar(T0 + 3 * MIN)], PROV); // 缺口 T0+2min
  const ts = await s.timestamps('BTCUSDT', '1m', T0, T0 + 3 * MIN);
  assert.deepEqual(ts, [T0, T0 + MIN, T0 + 3 * MIN]);
  ts.forEach((t) => assert.equal(typeof t, 'number'));
  s.close();
});

test('namespace isolates imported data from exchange bronze', async () => {
  const s = await freshStore();
  await s.upsertBars([bar(T0, 100)], { source: 'ccxt:binance', namespace: 'bronze' });
  await s.upsertBars([bar(T0, 555)], { source: 'csv-import', namespace: 'imported' });
  assert.equal(await s.countBars(), 2, '同 ts 不同 namespace 是两行');
  const bronze = await s.queryBars({ symbol: 'BTCUSDT', interval: '1m', namespace: 'bronze' });
  assert.equal(bronze.length, 1);
  assert.equal(bronze[0].close, 100);
  s.close();
});

// ── 只读护栏 ──

test('runReadOnly allows SELECT and returns columns + rows', async () => {
  const s = await freshStore();
  await s.upsertBars([bar(T0, 100)], PROV);
  const r = await s.runReadOnly("SELECT symbol, close FROM bars WHERE symbol = 'BTCUSDT'");
  assert.equal(r.status, 'ok', `${r.code ?? ''} ${r.reason ?? r.message ?? 'unexpected status'}`);
  assert.deepEqual(r.columns, ['symbol', 'close']);
  assert.equal(r.rows[0].close, 100);
  assert.equal(typeof r.rows[0].close, 'number');
  s.close();
});

test('runReadOnly blocks DML/DDL and multi-statement', async () => {
  const s = await freshStore();
  await s.upsertBars([bar(T0, 100)], PROV);

  for (const sql of [
    "DELETE FROM bars",
    "DROP TABLE bars",
    "UPDATE bars SET close = 0",
    "INSERT INTO bars VALUES ('X','1m',1,1,1,1,1,1,1,TRUE,'s','bronze',1)",
    "SELECT * FROM bars; DROP TABLE bars",
  ]) {
    const r = await s.runReadOnly(sql);
    assert.equal(r.status, 'blocked', `应被拦截: ${sql}`);
  }
  assert.equal(await s.countBars(), 1, '被拦截的语句不得产生任何影响');
  s.close();
});

test('runReadOnly auto-tightens LIMIT to maxRows', async () => {
  const s = await freshStore();
  await s.upsertBars(Array.from({ length: 50 }, (_, i) => bar(T0 + i * MIN)), PROV);
  const r = await s.runReadOnly('SELECT * FROM bars', { maxRows: 10 });
  assert.equal(r.status, 'ok');
  assert.equal(r.rows.length, 10, '无 LIMIT 的查询必须被自动收紧');
  assert.equal(r.truncated, true);
  s.close();
});

test('runReadOnly does not false-positive on column names containing keywords', async () => {
  const s = await freshStore();
  await s.upsertBars([bar(T0, 100)], PROV);
  // updated_at / deleted_flag 这类列名含关键字词根，护栏必须放行（整词匹配）
  await s.runReadOnly('SELECT symbol AS updated_at, close AS created_at FROM bars');
  const r = await s.runReadOnly('SELECT symbol, close FROM bars');
  assert.equal(r.status, 'ok');
  s.close();
});

test('schema browser lists tables and columns', async () => {
  const s = await freshStore();
  const tables = await s.tableNames();
  assert.ok(tables.includes('bars'));
  const cols = await s.describe('bars');
  const names = cols.map((c) => c.column);
  assert.ok(names.includes('ts'));
  assert.ok(names.includes('close'));
  assert.ok(names.includes('namespace'));
  s.close();
});

test('persistent file store survives reopen', async () => {
  const file = 'tests/.tmp/test-persist.duckdb';
  const s1 = await MarketStore.open(file);
  await s1.upsertBars([bar(T0, 42)], PROV);
  s1.close();

  const s2 = await MarketStore.open(file);
  const rows = await s2.queryBars({ symbol: 'BTCUSDT', interval: '1m' });
  assert.equal(rows.length, 1);
  assert.equal(rows[0].close, 42, '重开后数据应在');
  s2.close();
});
