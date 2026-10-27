// Data contract — enforces the three columns that MUST exist from day 1 of the
// schema (QUANTDSH_DATA_RESEARCH.local.md §6.4 / data-layer conclusion):
//   as_of      研究截点（PIT 可复现性的前提）
//   ingested_at 入库时间（隔离"数据生成时间"与"我们拿到的时间"）
//   closed     该 bar 是否已收盘（隔离未来函数：未收盘的 1m bar 不得进入回测）
//
// These cannot be backfilled without re-pulling all history, so the contract is
// checked at ingest time and at query time. A record missing any of them is
// rejected (fail-closed), never silently zero-filled (QuantStudio's "save
// overwrites, no schema version" trap — §4.5).

export interface DatasetContract {
  readonly asOf: number | null; // ms epoch, research cutoff
  readonly ingestedAt: number | null; // ms epoch, when we stored it
  readonly closedOnly: boolean; // does this dataset expose only closed bars?
}

export interface ContractCheck {
  readonly valid: boolean;
  readonly missing: readonly string[]; // which mandatory keys are absent/invalid
  readonly reason: string | null;
}

const MANDATORY: ReadonlyArray<keyof DatasetContract> = ['asOf', 'ingestedAt', 'closedOnly'];

/** Validate a dataset record against the mandatory contract. */
export function validateContract(record: Partial<DatasetContract> | null | undefined): ContractCheck {
  if (record === null || record === undefined) {
    return Object.freeze({
      valid: false,
      missing: Object.freeze([...MANDATORY]),
      reason: 'contract/empty-record',
    });
  }
  const missing: string[] = [];
  for (const key of MANDATORY) {
    const value = record[key];
    if (value === undefined || value === null) {
      missing.push(key);
      continue;
    }
    if ((key === 'asOf' || key === 'ingestedAt') && !Number.isFinite(value as number)) {
      missing.push(key);
    }
    if (key === 'closedOnly' && typeof value !== 'boolean') {
      missing.push(key);
    }
  }
  const valid = missing.length === 0;
  return Object.freeze({
    valid,
    missing: Object.freeze(missing),
    reason: valid ? null : `contract/missing:${missing.join(',')}`,
  });
}
