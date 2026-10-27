/**
 * K 线指标计算 —— 纯函数，不依赖 React / DOM。
 *
 * 全部返回与输入等长的数组，**不足周期处为 null**（而不是压缩数组），
 * 这样画图时可以直接用同一个下标取 bar 与指标，不需要维护偏移量。
 */

export type Series = (number | null)[];

/** 简单移动平均。 */
export function sma(values: number[], period: number): Series {
  const out: Series = new Array(values.length).fill(null);
  if (period <= 0) return out;
  let sum = 0;
  for (let i = 0; i < values.length; i += 1) {
    sum += values[i];
    if (i >= period) sum -= values[i - period];
    if (i >= period - 1) out[i] = sum / period;
  }
  return out;
}

/**
 * 指数移动平均。前 `period - 1` 个点没有定义，用前 period 个点的 SMA 作为种子
 * （与多数行情软件一致；用 0 起递推会在开头拉出一条假斜率）。
 */
export function ema(values: number[], period: number): Series {
  const out: Series = new Array(values.length).fill(null);
  if (period <= 0 || values.length < period) return out;
  const k = 2 / (period + 1);
  let seed = 0;
  for (let i = 0; i < period; i += 1) seed += values[i];
  let prev = seed / period;
  out[period - 1] = prev;
  for (let i = period; i < values.length; i += 1) {
    prev = values[i] * k + prev * (1 - k);
    out[i] = prev;
  }
  return out;
}

export interface BollBands {
  upper: Series;
  middle: Series;
  lower: Series;
}

/** 布林带（默认 20 周期 ± 2 倍标准差）。 */
export function boll(closes: number[], period = 20, mult = 2): BollBands {
  const middle = sma(closes, period);
  const upper: Series = new Array(closes.length).fill(null);
  const lower: Series = new Array(closes.length).fill(null);
  for (let i = 0; i < closes.length; i += 1) {
    const mid = middle[i];
    if (mid == null) continue;
    let acc = 0;
    for (let j = i - period + 1; j <= i; j += 1) acc += (closes[j] - mid) ** 2;
    const sd = Math.sqrt(acc / period);
    upper[i] = mid + mult * sd;
    lower[i] = mid - mult * sd;
  }
  return { upper, middle, lower };
}

export interface MacdSeries {
  dif: Series;
  dea: Series;
  hist: Series;
}

/** MACD(fast, slow, signal)：柱为 (DIF - DEA) × 2，与国内行情软件口径一致。 */
export function macd(closes: number[], fast = 12, slow = 26, signal = 9): MacdSeries {
  const fastLine = ema(closes, fast);
  const slowLine = ema(closes, slow);
  const dif: Series = closes.map((_, i) => {
    const f = fastLine[i];
    const s = slowLine[i];
    return f == null || s == null ? null : f - s;
  });
  // DEA 只对 DIF 有值的片段做 EMA，避免 null 段参与递推。
  const start = dif.findIndex((v) => v != null);
  const dea: Series = new Array(closes.length).fill(null);
  if (start >= 0) {
    const compact = dif.slice(start).map((v) => v ?? 0);
    const compactDea = ema(compact, signal);
    for (let i = 0; i < compactDea.length; i += 1) dea[start + i] = compactDea[i];
  }
  const hist: Series = closes.map((_, i) => {
    const d = dif[i];
    const e = dea[i];
    return d == null || e == null ? null : (d - e) * 2;
  });
  return { dif, dea, hist };
}

/** 涨跌幅是否为正（用于红绿判定）。 */
export function isUp(open: number, close: number): boolean {
  return close >= open;
}
