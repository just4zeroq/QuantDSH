import test from 'node:test';
import assert from 'node:assert/strict';
import { createRequire } from 'node:module';
import { fileURLToPath } from 'node:url';

const require = createRequire(new URL('../../../../apps/web/package.json', import.meta.url));
const { build } = require('esbuild');
const built = await build({
  stdin: { contents: "export * from './symbol';", resolveDir: fileURLToPath(new URL('../src/domain/', import.meta.url)), loader: 'ts' },
  bundle: true, platform: 'node', format: 'esm', write: false,
});
const { normalizeSymbol, symbolPartition, SymbolParseError } = await import(`data:text/javascript;base64,${Buffer.from(built.outputFiles[0].text).toString('base64')}`);

test('binance perp normalizes to canonical base+quote', () => {
  const s = normalizeSymbol('btcusdt', 'binance');
  assert.equal(s.exchange, 'binance');
  assert.equal(s.base, 'BTC');
  assert.equal(s.quote, 'USDT');
  assert.equal(s.type, 'perp');
  assert.equal(s.canonical, 'binance:BTCUSDT');
  assert.equal(symbolPartition(s.canonical), 'symbol=binance_BTCUSDT');
});

test('binance USD-margined future is typed future, not perp', () => {
  const s = normalizeSymbol('BTCUSD', 'binance');
  assert.equal(s.type, 'future');
  assert.equal(s.canonical, 'binance:BTCUSD');
});

test('okx swap and spot parse distinctly', () => {
  const swap = normalizeSymbol('BTC-USDT-SWAP', 'okx');
  assert.equal(swap.type, 'perp');
  assert.equal(swap.canonical, 'okx:BTC-USDT');
  const spot = normalizeSymbol('BTC-USDT', 'okx');
  assert.equal(spot.type, 'spot');
  const future = normalizeSymbol('BTC-USDT-2506', 'okx');
  assert.equal(future.type, 'future');
});

test('unparseable input fails closed', () => {
  assert.throws(() => normalizeSymbol('', 'binance'), SymbolParseError);
  assert.throws(() => normalizeSymbol('BTC', 'binance'), SymbolParseError);
  assert.throws(() => normalizeSymbol('NOTABASE-QUOTE', 'okx'), SymbolParseError);
});

test('same underlying on different exchanges stays distinct', () => {
  // This is the trap QuantMind fell into (single-key `symbol` table).
  const a = normalizeSymbol('BTCUSDT', 'binance');
  const o = normalizeSymbol('BTC-USDT-SWAP', 'okx');
  assert.notEqual(a.canonical, o.canonical);
});
