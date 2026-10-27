// Freshness grading — answers "how stale is this dataset?" with a single enum
// that BOTH the UI badge and the agent tool read from. The page must never
// compose its own freshness string (QUANTDSH_DATA_RESEARCH.local.md §7 约束 2):
// only `gradeFreshness` produces the level, only `freshnessPresentation`
// produces the label/tone. This keeps "what the user sees" == "what the AI sees".

export type FreshnessLevel = 'fresh' | 'lagging' | 'stale' | 'gapped' | 'empty';

export interface Freshness {
  readonly level: FreshnessLevel;
  readonly latestBarTs: number | null; // ms epoch of the newest closed bar, or null
  readonly nowTs: number; // ms epoch of the reference "now"
  readonly lagMs: number; // nowTs - latestBarTs (0 when null)
  readonly lagBars: number; // lagMs / intervalMs, rounded up
}

// Crypto trades 24/7, so the only sane freshness reference is wall-clock time,
// not "trading sessions since". Intervals are in milliseconds.
const MINUTE = 60_000;
const HOUR = 60 * MINUTE;

// Tunables: how many bar-intervals of lag map to each level.
const FRESH_WITHIN = 1.5; // <= 1.5 intervals => fresh
const LAGGING_WITHIN = 3; // <= 3 intervals => lagging
const STALE_WITHIN = 24 * HOUR; // <= 24h => stale (else gapped)

/**
 * Grade a dataset's freshness from its newest closed bar and the current time.
 * `intervalMs` is the bar size (e.g. 60000 for 1m, 86400000 for 1d).
 */
export function gradeFreshness(
  latestBarTs: number | null,
  nowTs: number,
  intervalMs: number,
): Freshness {
  if (!Number.isFinite(nowTs) || !Number.isFinite(intervalMs) || intervalMs <= 0) {
    throw new Error('freshness/invalid-input');
  }
  if (latestBarTs === null) {
    return Object.freeze({
      level: 'empty',
      latestBarTs: null,
      nowTs,
      lagMs: 0,
      lagBars: 0,
    });
  }
  if (!Number.isFinite(latestBarTs)) throw new Error('freshness/invalid-input');
  const lagMs = Math.max(0, nowTs - latestBarTs);
  const lagBars = Math.ceil(lagMs / intervalMs);
  let level: FreshnessLevel;
  if (lagMs <= FRESH_WITHIN * intervalMs) level = 'fresh';
  else if (lagMs <= LAGGING_WITHIN * intervalMs) level = 'lagging';
  else if (lagMs <= STALE_WITHIN) level = 'stale';
  else level = 'gapped';
  return Object.freeze({ level, latestBarTs, nowTs, lagMs, lagBars });
}

export type FreshnessTone = 'ok' | 'warn' | 'alert' | 'muted';

export interface FreshnessPresentation {
  readonly text: string; // human label, also consumed by agent tools
  readonly tone: FreshnessTone; // drives badge color in both UI and tool output
}

/** Pure mapping level -> presentation. No I/O, no Date.now(). */
export function freshnessPresentation(f: Freshness): FreshnessPresentation {
  const mins = Math.round(f.lagMs / MINUTE);
  switch (f.level) {
    case 'fresh':
      return Object.freeze({ text: '实时', tone: 'ok' });
    case 'lagging':
      return Object.freeze({ text: `滞后 ${mins} 分钟`, tone: 'warn' });
    case 'stale':
      return Object.freeze({ text: `陈旧 ${mins} 分钟`, tone: 'alert' });
    case 'gapped':
      return Object.freeze({ text: '数据断层', tone: 'alert' });
    case 'empty':
      return Object.freeze({ text: '无数据', tone: 'muted' });
  }
}
