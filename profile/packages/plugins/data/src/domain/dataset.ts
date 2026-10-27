// Dataset record & summary — the shared shape for "one time series we store".
// A DatasetRecord is what the runtime layer (engine / storage) produces; the page
// and the agent tool only ever read the derived DatasetSummary, which is computed
// HERE from the pure domain functions. No component re-derives freshness or
// coverage (QUANTDSH_DATA_RESEARCH.local.md §7 约束 2).

import type { Exchange } from './symbol';
import type { DatasetContract } from './contract';
import { gradeFreshness, type Freshness } from './freshness';
import { detectGaps, type GapReport } from './coverage';
import { validateContract, type ContractCheck } from './contract';

// Market / Interval / INTERVAL_MS 统一定义在 ./market（宽版：四类市场 + 9 个周期）。
// 这里刻意**不再**重新导出它们 —— 否则 domain/index.ts 的 `export *` 会因重名报 TS2308。
// 窄版（'crypto' / '1m'|'1d'）是 v1 初期的遗留，已由宽版取代：
// UI 筛选壳要显示四类市场（§6.1.1），把类型钉死在 crypto 会让扩展市场时处处改。
import { INTERVAL_MS } from './market';
import type { Interval, Market } from './market';

export interface DatasetRecord {
  readonly id: string;
  readonly market: Market;
  readonly exchange: Exchange;
  readonly symbol: string; // canonical, e.g. binance:BTCUSDT
  readonly interval: Interval;
  readonly latestBarTs: number | null; // newest closed bar, ms epoch
  readonly startTs: number; // first expected bar, ms epoch
  readonly endTs: number; // last expected bar, ms epoch (inclusive)
  readonly presentBars: readonly number[]; // timestamps actually stored
  readonly contract: DatasetContract;
}

export interface DatasetSummary {
  readonly record: DatasetRecord;
  readonly freshness: Freshness;
  readonly coverage: GapReport;
  readonly contract: ContractCheck;
}

/** Derive the full summary a page/agent reads, from one record. Pure. */
export function summarizeDataset(record: DatasetRecord, nowTs: number): DatasetSummary {
  const intervalMs = INTERVAL_MS[record.interval];
  return Object.freeze({
    record,
    freshness: gradeFreshness(record.latestBarTs, nowTs, intervalMs),
    coverage: detectGaps(record.startTs, record.endTs, intervalMs, record.presentBars),
    contract: validateContract(record.contract),
  });
}
