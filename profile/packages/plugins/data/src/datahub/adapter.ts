/**
 * 守护进程线格式 → domain 的 DatasetRecord。
 *
 * 这一步存在的原因：domain 层（新鲜度 / 覆盖率 / 契约的判定）是全项目唯一实现，
 * 页面和 Agent 工具都必须消费它。守护进程返回的是存储层的行，不能直接当
 * DatasetRecord 用 —— 适配必须发生在边界上，且只发生一次。
 *
 * 关键映射：
 *  - as_of  → latestBarTs（最新已收盘 bar 的时间，新鲜度的唯一输入）
 *  - 缺失判定 → 由该月的时间戳数组经 domain.detectGaps 算出，
 *    **不用** row_count 反推：行数相同但中间有洞是最常见的故障形态。
 *
 * 本文件刻意**不 import react**：host 侧（Node 主进程）与 client 侧（浏览器 bundle）
 * 共用同一份适配，否则主进程打包会把 React 一起拖进来。
 */

import { createDatahubClient, resolveEndpoint, type DatasetWire, type DatahubClient, type DatahubEndpoint, type SyncJobWire } from './client';
import type { DatasetRecord, Interval, Market } from '../domain';
import { MARKETS, intervalMs } from '../domain';
import { normalizeSymbol, type Exchange } from '../domain/symbol';

/** 一个自然月的 [起, 止]（UTC）。覆盖率网格按整月算，边界才不会随刷新时间漂移。 */
export function monthWindow(refTs: number): { year: number; month: number; from: number; to: number } {
  const d = new Date(refTs);
  const year = d.getUTCFullYear();
  const month = d.getUTCMonth() + 1;
  return { year, month, from: Date.UTC(year, month - 1, 1), to: Date.UTC(year, month, 1) - 1 };
}

export function asMarket(raw: string): Market {
  return (MARKETS as readonly string[]).includes(raw) ? (raw as Market) : 'CRYPTO';
}

export function asInterval(raw: string, fallback: Interval = '1m'): Interval {
  try {
    intervalMs(raw);
    return raw as Interval;
  } catch {
    // 未知周期不能让整个目录崩：降级到 1m 并让该条目的覆盖率明显偏低，
    // 好过整个页面白屏（周期非法是数据问题，不是页面问题）。
    return fallback;
  }
}

export function canonicalSymbol(symbol: string, exchange: string): string {
  try {
    return normalizeSymbol(symbol, exchange as Exchange).canonical;
  } catch {
    return `${exchange}:${symbol}`;
  }
}

export interface DatahubPayload {
  datasets: DatasetRecord[];
  jobs: SyncJobWire[];
}

async function toDatasetRecord(
  row: DatasetWire,
  client: DatahubClient,
  nowTs: number,
): Promise<DatasetRecord> {
  const interval = asInterval(row.interval);
  // 以数据集自己的最新 bar 定位月份，而不是用页面时钟：
  // 日线数据的最后一根可能在上个月，按当前月取会得出"覆盖率 0"的假结论。
  const win = monthWindow(row.as_of ?? nowTs);
  let presentBars: readonly number[] = [];
  try {
    presentBars = await client.timestamps({
      symbol: row.symbol,
      interval,
      namespace: row.namespace,
      from: win.from,
      to: win.to,
    });
  } catch {
    presentBars = []; // 单个数据集取不到不该拖垮整个目录
  }
  return {
    id: row.dataset_key,
    market: asMarket(row.market),
    exchange: row.exchange as DatasetRecord['exchange'],
    symbol: canonicalSymbol(row.symbol, row.exchange),
    interval,
    latestBarTs: row.as_of,
    startTs: win.from,
    endTs: win.to,
    presentBars,
    contract: { asOf: row.as_of, ingestedAt: row.ingested_at, closedOnly: row.closed_only },
  };
}

/** 装载目录 + 同步任务。供 Agent 工具与非 React 调用方使用。 */
export async function loadDatasetsFromDatahub(
  nowTs: number,
  endpoint: DatahubEndpoint = resolveEndpoint(),
): Promise<DatahubPayload> {
  const client = createDatahubClient(endpoint);
  const rows = await client.datasets();
  const datasets: DatasetRecord[] = [];
  for (const row of rows) {
    datasets.push(await toDatasetRecord(row, client, nowTs));
  }
  let jobs: SyncJobWire[] = [];
  try {
    jobs = await client.jobs();
  } catch {
    jobs = [];
  }
  return { datasets, jobs };
}
