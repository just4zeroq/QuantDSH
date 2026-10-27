// Presentation layer — the ONLY place that turns a DatasetSummary into display
// strings / tones. The page renders from these props; it never builds a
// freshness label or coverage % inline. This guarantees "what the user sees" ==
// "what an agent tool reports" (QUANTDSH_DATA_RESEARCH.local.md §7 约束 2).

import type { DatasetSummary, Interval } from '../domain';
import { freshnessPresentation, type FreshnessTone } from '../domain';

export interface DatasetRowProps {
  readonly id: string;
  readonly symbol: string;
  readonly interval: Interval;
  readonly freshnessText: string;
  readonly freshnessTone: FreshnessTone;
  readonly coverageText: string; // e.g. "90%"
  readonly coverageTone: 'ok' | 'warn' | 'alert';
  readonly contractValid: boolean;
}

export interface DatasetDetailProps {
  readonly symbol: string;
  readonly interval: Interval;
  readonly freshnessText: string;
  readonly freshnessTone: FreshnessTone;
  readonly expectedBars: number;
  readonly presentBars: number;
  readonly missingBars: number;
  readonly coverageText: string;
  readonly coverageTone: 'ok' | 'warn' | 'alert';
  readonly contractText: string;
  readonly contractValid: boolean;
  readonly source: string; // exchange + market, from contract-as-source convention
}

export function formatPercent(ratio: number): string {
  if (!Number.isFinite(ratio)) return '—';
  return `${Math.round(ratio * 100)}%`;
}

export function coverageTone(ratio: number): 'ok' | 'warn' | 'alert' {
  if (ratio >= 0.999) return 'ok';
  if (ratio >= 0.9) return 'warn';
  return 'alert';
}

export function toRowProps(summary: DatasetSummary): DatasetRowProps {
  const { freshness, coverage, record, contract } = summary;
  const f = freshnessPresentation(freshness);
  return {
    id: record.id,
    symbol: record.symbol,
    interval: record.interval,
    freshnessText: f.text,
    freshnessTone: f.tone,
    coverageText: formatPercent(coverage.coverageRatio),
    coverageTone: coverageTone(coverage.coverageRatio),
    contractValid: contract.valid,
  };
}

export function toDetailProps(summary: DatasetSummary): DatasetDetailProps {
  const { freshness, coverage, record, contract } = summary;
  const f = freshnessPresentation(freshness);
  return {
    symbol: record.symbol,
    interval: record.interval,
    freshnessText: f.text,
    freshnessTone: f.tone,
    expectedBars: coverage.expected,
    presentBars: coverage.present,
    missingBars: coverage.missing.length,
    coverageText: formatPercent(coverage.coverageRatio),
    coverageTone: coverageTone(coverage.coverageRatio),
    contractText: contract.valid ? '契约完整' : `缺 ${contract.missing.join('/')}`,
    contractValid: contract.valid,
    source: `${record.exchange} · ${record.market}`,
  };
}
