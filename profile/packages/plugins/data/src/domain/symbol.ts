// Symbol normalization — the single source of truth for "what is this instrument".
// Both the data page and the market (quote) page MUST route through this module;
// never parse a symbol string inline in a component or an agent tool.
//
// Design rules (from QUANTDSH_DATA_RESEARCH.local.md §5 缺陷三):
//  - crypto has no split/limit/adjustment, but Binance vs OKX name the SAME
//    perpetual differently. We canonicalize to `{exchange}:{base}{quote}` so a
//    Binance BTC perpetual and an OKX BTC perpetual stay distinct instruments.
//  - Fail closed on unparseable input instead of guessing (QuantDinger's 5-way
//   ad-hoc normalization is the anti-pattern we are removing).

export type Exchange = 'binance' | 'okx';
export type InstrumentType = 'spot' | 'perp' | 'future';

export interface NormalizedSymbol {
  readonly raw: string;
  readonly exchange: Exchange;
  readonly base: string;
  readonly quote: string;
  readonly type: InstrumentType;
  readonly canonical: string; // stable key for storage / joins
}

const BASE_QUOTE_RE = /^([A-Z0-9]{2,12})(USDT|USDC|BUSD|USD|TUSD|DAI)$/;
// OKX spot style BTC-USDT, perpetual BTC-USDT-SWAP, future BTC-USDT-2506
const OKX_RE = /^([A-Z0-9]{2,12})-([A-Z]{3,4})(?:-(SWAP|\d{4}))?$/;

export class SymbolParseError extends Error {
  constructor(public readonly raw: string, reason: string) {
    super(`symbol/parse: ${raw} (${reason})`);
    this.name = 'SymbolParseError';
  }
}

/** Parse a raw symbol from a known exchange into a canonical, typed form. */
export function normalizeSymbol(raw: string, exchange: Exchange): NormalizedSymbol {
  const value = raw.trim();
  if (!value) throw new SymbolParseError(raw, 'empty');
  if (exchange === 'binance') return parseBinance(value, raw);
  return parseOkx(value, raw);
}

function parseBinance(value: string, raw: string): NormalizedSymbol {
  const m = BASE_QUOTE_RE.exec(value.toUpperCase());
  if (!m) throw new SymbolParseError(raw, 'not base+quote');
  const base = m[1];
  const quote = m[2];
  const type: InstrumentType = quote === 'USD' ? 'future' : 'perp';
  return Object.freeze({
    raw,
    exchange: 'binance',
    base,
    quote,
    type,
    canonical: `binance:${base}${quote}`,
  });
}

function parseOkx(value: string, raw: string): NormalizedSymbol {
  const m = OKX_RE.exec(value.toUpperCase());
  if (!m) throw new SymbolParseError(raw, 'not base-quote[-suffix]');
  const base = m[1];
  const quote = m[2];
  const suffix = m[3];
  const type: InstrumentType = !suffix ? 'spot' : suffix === 'SWAP' ? 'perp' : 'future';
  return Object.freeze({
    raw,
    exchange: 'okx',
    base,
    quote,
    type,
    // OKX already uses the `BASE-QUOTE` convention; keep it for readability.
    canonical: `okx:${base}-${quote}`,
  });
}

/** Build the partition path segment for storage (hive-style `symbol=...`). */
export function symbolPartition(canonical: string): string {
  return `symbol=${canonical.replace(/[:]/g, '_')}`;
}
