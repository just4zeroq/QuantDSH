// Market & interval vocabulary — shared by the data page, the market (quote) page,
// AND the engine. Must stay dependency-free: the React client bundles this module,
// so it must never transitively import `node:*` (the engine does, the client can't).
//
// Why the four markets exist in the UI but only crypto has a live source:
// v1 ships a crypto-only pipeline; A-share/US/HK render as "source not connected".
// The enum is deliberately complete from day one so adding a market later is
// "add an adapter", not "change every switch statement" (§6.1.1 扩展性抽象 #1).

export type Market = 'CRYPTO' | 'A_SHARE' | 'US' | 'HK';

export const MARKETS: readonly Market[] = Object.freeze(['CRYPTO', 'A_SHARE', 'US', 'HK']);

export const MARKET_LABEL: Readonly<Record<Market, string>> = Object.freeze({
  CRYPTO: '加密',
  A_SHARE: 'A股',
  US: '美股',
  HK: '港股',
});

/** 24/7 市场没有交易日历；有日历的市场（v1.5）必须显式提供日历，绝不能用 24/7 网格兜底。 */
export const MARKET_IS_24_7: Readonly<Record<Market, boolean>> = Object.freeze({
  CRYPTO: true,
  A_SHARE: false,
  US: false,
  HK: false,
});

export type Interval = '1m' | '3m' | '5m' | '15m' | '30m' | '1h' | '2h' | '4h' | '1d';

export const INTERVALS: readonly Interval[] = Object.freeze([
  '1m',
  '3m',
  '5m',
  '15m',
  '30m',
  '1h',
  '2h',
  '4h',
  '1d',
]);

export const INTERVAL_MS: Readonly<Record<Interval, number>> = Object.freeze({
  '1m': 60_000,
  '3m': 180_000,
  '5m': 300_000,
  '15m': 900_000,
  '30m': 1_800_000,
  '1h': 3_600_000,
  '2h': 7_200_000,
  '4h': 14_400_000,
  '1d': 86_400_000,
});

export function isInterval(v: string): v is Interval {
  return (INTERVALS as readonly string[]).includes(v);
}

/**
 * Interval → 毫秒。未知 interval 直接抛错（fail-closed），绝不默默按 1m 处理：
 * 静默降级会让覆盖率与质量检验把 1d 数据当成 1m 来算，缺口数字全错。
 */
export function intervalMs(i: string): number {
  const v = INTERVAL_MS[i as Interval];
  if (v === undefined) throw new Error(`interval/unknown: ${i}`);
  return v;
}

/** 把毫秒周期反查回 interval 名（用于从数据推断周期）；非标准值返回 null。 */
export function intervalOf(ms: number): Interval | null {
  for (const k of INTERVALS) if (INTERVAL_MS[k] === ms) return k;
  return null;
}

/** 一个自然日内应有的 bar 数（24/7 市场）。用于覆盖率 expected 计算。 */
export function barsPerDay(i: string): number {
  return Math.round(86_400_000 / intervalMs(i));
}
