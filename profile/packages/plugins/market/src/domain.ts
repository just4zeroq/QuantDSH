// Market / interval vocabulary + the bar shape the chart consumes.
//
// Kept deliberately small and self-contained: the market (quote) page is a
// SEPARATE workbench package from the data workbench (the host registry allows
// exactly one workbench descriptor per plugin package, and the installed
// manifest's descriptor must match what the client declares byte-for-byte).
// It still talks to the SAME Rust daemon (quantdsh-datahub) over the SAME HTTP
// contract — so the bars it draws are exactly what the data workbench audits.
//
// Vela's offline `data` bar shape is `{ time, open, high, low, close, volume? }`
// with `time` = bar OPEN time in epoch MILLISECONDS. Our wire bars use `ts`
// (ms) + OHLCV, so the adapter in MarketPage maps ts → time.

export type Market = 'CRYPTO' | 'A_SHARE' | 'US' | 'HK';

export const MARKETS: readonly Market[] = Object.freeze(['CRYPTO', 'A_SHARE', 'US', 'HK']);

export const MARKET_LABEL: Readonly<Record<Market, string>> = Object.freeze({
  CRYPTO: '加密货币',
  A_SHARE: 'A股',
  US: '美股',
  HK: '港股',
});

export type Interval = '1m' | '3m' | '5m' | '15m' | '30m' | '1h' | '2h' | '4h' | '1d';

export const INTERVALS: readonly Interval[] = Object.freeze([
  '1m', '3m', '5m', '15m', '30m', '1h', '2h', '4h', '1d',
]);

/** Map our interval token to the timeframe string Vela expects. */
export function toVelaTimeframe(interval: Interval): string {
  // Vela uses '1D' for daily; everything else matches our intraday tokens.
  if (interval === '1d') return '1D';
  return interval;
}

/** A single OHLCV bar in the shape Vela's offline `data` option consumes. */
export interface ChartBar {
  time: number; // epoch ms, bar OPEN time
  open: number;
  high: number;
  low: number;
  close: number;
  volume?: number;
}
