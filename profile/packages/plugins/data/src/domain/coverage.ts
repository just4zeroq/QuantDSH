// Coverage — given a time range and the bars we actually have, report what's
// missing. This is the engine behind the "coverage panel" (day x hour heatmap
// where missing cells are left blank) and the agent "is this range covered?"
// check. Pure & deterministic: no file access, no clock.
//
// v1 scope is crypto 1m/daily (QUANTDSH_DATA_RESEARCH.local.md §8.1), which
// trades 24/7, so `expectedBars` is a continuous grid. A trading-calendar
// variant (A-share / US) is a future extension and must NOT silently fill
// weekends with phantom bars.

export interface GapReport {
  readonly startTs: number;
  readonly endTs: number;
  readonly intervalMs: number;
  readonly expected: number; // count of bars that should exist
  readonly present: number; // count of bars we actually have
  readonly missing: readonly number[]; // timestamps that are expected but absent
  readonly coverageRatio: number; // present / expected, 0..1
}

/**
 * Continuous expected timestamps for a 24/7 market, aligned to interval
 * boundaries. `startTs`/`endTs` are inclusive bounds; we snap them outward to
 * the nearest interval grid so the report never claims a partial bar is missing.
 */
export function expectedBars(startTs: number, endTs: number, intervalMs: number): readonly number[] {
  if (!Number.isFinite(startTs) || !Number.isFinite(endTs) || !Number.isFinite(intervalMs) || intervalMs <= 0) {
    throw new Error('coverage/invalid-input');
  }
  // 返回 readonly：调用方不该改写"期望网格"，它是派生事实不是可变状态
  if (endTs < startTs) return Object.freeze([] as number[]);
  const first = Math.floor(startTs / intervalMs) * intervalMs;
  const last = Math.floor(endTs / intervalMs) * intervalMs;
  const out: number[] = [];
  for (let t = first; t <= last; t += intervalMs) out.push(t);
  return Object.freeze(out);
}

/** Compare expected timestamps against the actual set; return what's missing. */
export function detectGaps(
  startTs: number,
  endTs: number,
  intervalMs: number,
  actual: readonly number[],
): GapReport {
  const expected = expectedBars(startTs, endTs, intervalMs);
  const have = new Set(actual);
  const missing = expected.filter((t) => !have.has(t));
  const present = expected.length - missing.length;
  const coverageRatio = expected.length === 0 ? 1 : present / expected.length;
  return Object.freeze({
    startTs,
    endTs,
    intervalMs,
    expected: expected.length,
    present,
    missing: Object.freeze(missing),
    coverageRatio,
  });
}

export interface CoverageCell {
  readonly day: string; // YYYY-MM-DD
  readonly hour: number; // 0..23
  readonly present: number; // bars present in this hour
  readonly expected: number; // bars expected in this hour (interval-dependent)
  readonly ratio: number;
}

/**
 * Build a day x hour matrix for a month window — the data structure behind the
 * coverage heatmap. Missing hours are reported with present=0 (not skipped), so
 * the UI can render empty cells instead of inventing data.
 */
export function coverageMatrix(
  year: number,
  month: number, // 1-based
  intervalMs: number,
  actual: readonly number[],
): readonly CoverageCell[] {
  if (!Number.isFinite(intervalMs) || intervalMs <= 0) throw new Error('coverage/invalid-input');
  const barsPerHour = Math.max(1, Math.round(3_600_000 / intervalMs));
  const have = new Set(actual);
  const cells: CoverageCell[] = [];
  const daysInMonth = new Date(Date.UTC(year, month, 0)).getUTCDate();
  for (let day = 1; day <= daysInMonth; day++) {
    for (let hour = 0; hour < 24; hour++) {
      let present = 0;
      const hourStart = Date.UTC(year, month - 1, day, hour, 0, 0);
      const firstBar = Math.floor(hourStart / intervalMs) * intervalMs;
      for (let b = 0; b < barsPerHour; b++) {
        const ts = firstBar + b * intervalMs;
        if (have.has(ts)) present++;
      }
      cells.push(
        Object.freeze({
          day: `${year}-${String(month).padStart(2, '0')}-${String(day).padStart(2, '0')}`,
          hour,
          present,
          expected: barsPerHour,
          ratio: present / barsPerHour,
        }),
      );
    }
  }
  return Object.freeze(cells);
}
