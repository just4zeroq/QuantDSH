//! Data source adapters.
//!
//! The `DataSource` enum is the extension seam promised in the v1 spec: adding
//! a market means adding one variant and one branch in [`resolve`] — the sync
//! job schema, the runner and the pages stay untouched.
//!
//! Two of the three sources need no network, which is what makes the sync
//! pipeline testable offline:
//!   * `replay`  — deterministic synthetic bars (same input ⇒ same output)
//!   * `binance` / `okx` — real REST klines
//!
//! Note there is deliberately **no CCXT-style universal layer**. CCXT is a
//! crypto library; it would be wrong to pretend it also covers equities later.
//! Each venue gets an explicit adapter with its own symbol and payload shape.

use crate::error::{AppError, AppResult};
use crate::market::Bar;

#[derive(Debug, Clone)]
pub struct FetchRequest {
    pub symbol: String,
    pub interval: String,
    /// Inclusive lower bound in ms.
    pub from: i64,
    /// Inclusive upper bound in ms.
    pub to: i64,
    /// Hard cap on how many bars one call may return.
    pub limit: u64,
}

pub enum DataSource {
    Replay(ReplaySource),
    Binance(BinanceSource),
    Okx(OkxSource),
}

impl DataSource {
    pub fn name(&self) -> &'static str {
        match self {
            DataSource::Replay(_) => "replay",
            DataSource::Binance(_) => "binance",
            DataSource::Okx(_) => "okx",
        }
    }

    pub async fn fetch_bars(&self, req: &FetchRequest) -> AppResult<Vec<Bar>> {
        match self {
            DataSource::Replay(s) => s.fetch_bars(req),
            DataSource::Binance(s) => s.fetch_bars(req).await,
            DataSource::Okx(s) => s.fetch_bars(req).await,
        }
    }
}

/// Resolve a source name. Returns `None` for unknown names rather than falling
/// back to a default — silently substituting a source would write data whose
/// provenance is a lie.
pub fn resolve(name: &str) -> Option<DataSource> {
    match name.to_ascii_lowercase().as_str() {
        "replay" => Some(DataSource::Replay(ReplaySource)),
        "binance" => Some(DataSource::Binance(BinanceSource::new())),
        "okx" => Some(DataSource::Okx(OkxSource::new())),
        _ => None,
    }
}

pub const SOURCE_NAMES: [&str; 3] = ["replay", "binance", "okx"];

// ─────────────────────────────── Replay ───────────────────────────────

/// Deterministic synthetic bars. Used by tests and as an offline demo feed so
/// the whole sync → catalog → coverage → quality path can be exercised without
/// touching a network.
pub struct ReplaySource;

fn mix(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^ (x >> 31)
}

fn hash_str(s: &str) -> u64 {
    let mut h: u64 = 0xCBF2_9CE4_8422_2325;
    for b in s.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100_0000_01B3);
    }
    mix(h)
}

/// Stable pseudo-random in `[0, 1)` derived from (symbol, ts, channel).
fn unit(symbol: &str, ts: i64, channel: u64) -> f64 {
    let h = mix(hash_str(symbol) ^ (ts as u64).wrapping_mul(0x9E37_79B9) ^ (channel << 17));
    ((h >> 11) as f64) / (1u64 << 53) as f64
}

impl ReplaySource {
    pub fn fetch_bars(&self, req: &FetchRequest) -> AppResult<Vec<Bar>> {
        let step = crate::domain::interval_ms(&req.interval)?;
        if req.to < req.from {
            return Ok(Vec::new());
        }
        let first = req.from.div_euclid(step) * step;
        let last = req.to.div_euclid(step) * step;
        let base = 50.0 + (hash_str(&req.symbol) % 900) as f64;

        let mut out = Vec::new();
        let mut ts = first;
        while ts <= last {
            let open = base * (1.0 + unit(&req.symbol, ts, 1) * 0.02);
            let close = open * (1.0 + (unit(&req.symbol, ts, 2) - 0.5) * 0.01);
            let hi = open.max(close);
            let lo = open.min(close);
            let high = hi * (1.0 + unit(&req.symbol, ts, 3) * 0.005);
            let low = lo * (1.0 - unit(&req.symbol, ts, 4) * 0.005);
            let volume = 100.0 + unit(&req.symbol, ts, 5) * 900.0;

            out.push(Bar {
                symbol: req.symbol.clone(),
                interval: req.interval.clone(),
                ts,
                open,
                high,
                low,
                close,
                volume,
                turnover: volume * close,
                closed: true,
                source: "replay".into(),
                namespace: crate::market::DEFAULT_NAMESPACE.into(),
                ingested_at: 0,
            });
            ts += step;
            if out.len() as u64 >= req.limit {
                break;
            }
        }
        Ok(out)
    }
}

// ─────────────────────────────── Binance ───────────────────────────────

pub struct BinanceSource {
    base: String,
    client: reqwest::Client,
}

impl BinanceSource {
    pub fn new() -> Self {
        Self {
            base: "https://api.binance.com".into(),
            client: reqwest::Client::new(),
        }
    }

    /// Binance spells intervals the same way we do, so no translation table is
    /// needed — but validate anyway so a bad interval never reaches the venue.
    fn check(&self, interval: &str) -> AppResult<()> {
        if crate::domain::is_interval(interval) {
            Ok(())
        } else {
            Err(AppError::BadRequest(format!(
                "interval/unsupported-by-binance: {interval}"
            )))
        }
    }

    pub async fn fetch_bars(&self, req: &FetchRequest) -> AppResult<Vec<Bar>> {
        self.check(&req.interval)?;
        let url = format!("{}/api/v3/klines", self.base);
        let resp = self
            .client
            .get(url)
            .query(&[
                ("symbol", req.symbol.as_str()),
                ("interval", req.interval.as_str()),
                ("startTime", &req.from.to_string()),
                ("endTime", &req.to.to_string()),
                ("limit", &req.limit.to_string()),
            ])
            .send()
            .await
            .map_err(|e| AppError::Source(format!("binance request failed: {e}")))?;

        if !resp.status().is_success() {
            return Err(AppError::Source(format!(
                "binance http {}: {}",
                resp.status(),
                resp.text().await.unwrap_or_default()
            )));
        }
        let payload: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| AppError::Source(format!("binance bad json: {e}")))?;

        let arr = payload
            .as_array()
            .ok_or_else(|| AppError::Source("binance: expected an array".into()))?;

        let mut out = Vec::with_capacity(arr.len());
        for row in arr {
            let r = row
                .as_array()
                .ok_or_else(|| AppError::Source("binance: row is not an array".into()))?;
            if r.len() < 6 {
                continue;
            }
            out.push(Bar {
                symbol: req.symbol.clone(),
                interval: req.interval.clone(),
                ts: r[0].as_i64().unwrap_or(0),
                open: parse_num(&r[1])?,
                high: parse_num(&r[2])?,
                low: parse_num(&r[3])?,
                close: parse_num(&r[4])?,
                volume: parse_num(&r[5])?,
                turnover: 0.0,
                // Binance omits the confirm flag; a kline is only final once its
                // close time has passed, which `to` already bounds.
                closed: true,
                source: "binance".into(),
                namespace: crate::market::DEFAULT_NAMESPACE.into(),
                ingested_at: 0,
            });
        }
        Ok(out)
    }
}

impl Default for BinanceSource {
    fn default() -> Self {
        Self::new()
    }
}

// ─────────────────────────────── OKX ───────────────────────────────

pub struct OkxSource {
    base: String,
    client: reqwest::Client,
}

impl OkxSource {
    pub fn new() -> Self {
        Self {
            base: "https://www.okx.com".into(),
            client: reqwest::Client::new(),
        }
    }

    /// OKX uses `1m/5m/.../1H/1D` — capital letters for hour and day — and
    /// returns newest-first, so the caller's `from`/`to` map onto `after`/`before`.
    fn map_interval(i: &str) -> AppResult<&'static str> {
        Ok(match i {
            "1m" => "1m",
            "3m" => "3m",
            "5m" => "5m",
            "15m" => "15m",
            "30m" => "30m",
            "1h" => "1H",
            "2h" => "2H",
            "4h" => "4H",
            "1d" => "1D",
            other => {
                return Err(AppError::BadRequest(format!(
                    "interval/unsupported-by-okx: {other}"
                )))
            }
        })
    }

    /// OKX instrument ids look like `BTC-USDT` / `BTC-USDT-SWAP`; accept our
    /// `BTCUSDT` form by splitting on the quote asset.
    fn map_symbol(s: &str) -> String {
        for q in ["USDT", "USDC", "BTC", "ETH"] {
            if let Some(base) = s.strip_suffix(q) {
                if !base.is_empty() {
                    return format!("{base}-{q}");
                }
            }
        }
        s.to_string()
    }

    pub async fn fetch_bars(&self, req: &FetchRequest) -> AppResult<Vec<Bar>> {
        let bar = Self::map_interval(&req.interval)?;
        let inst = Self::map_symbol(&req.symbol);
        let resp = self
            .client
            .get(format!("{}/api/v5/market/history-candles", self.base))
            .query(&[
                ("instId", inst.as_str()),
                ("bar", bar),
                ("before", &req.from.to_string()),
                ("after", &req.to.to_string()),
                ("limit", "100"),
            ])
            .send()
            .await
            .map_err(|e| AppError::Source(format!("okx request failed: {e}")))?;

        let payload: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| AppError::Source(format!("okx bad json: {e}")))?;

        let code = payload.get("code").and_then(|c| c.as_str()).unwrap_or("");
        if code != "0" {
            return Err(AppError::Source(format!(
                "okx error {code}: {}",
                payload.get("msg").and_then(|m| m.as_str()).unwrap_or("")
            )));
        }
        let arr = payload
            .get("data")
            .and_then(|d| d.as_array())
            .ok_or_else(|| AppError::Source("okx: missing data array".into()))?;

        let mut out = Vec::with_capacity(arr.len());
        for row in arr {
            let r = match row.as_array() {
                Some(r) if r.len() >= 6 => r,
                _ => continue,
            };
            // OKX `confirm`: "0" not final, "1" final. Never write an unclosed
            // bar into bronze — its close/high/low are still moving.
            let closed = r
                .get(8)
                .and_then(|c| c.as_str())
                .map(|c| c == "1")
                .unwrap_or(true);
            if !closed {
                continue;
            }
            out.push(Bar {
                symbol: req.symbol.clone(),
                interval: req.interval.clone(),
                ts: r[0].as_str().and_then(|s| s.parse::<i64>().ok()).unwrap_or(0),
                open: parse_num(&r[1])?,
                high: parse_num(&r[2])?,
                low: parse_num(&r[3])?,
                close: parse_num(&r[4])?,
                volume: parse_num(&r[5])?,
                turnover: 0.0,
                closed,
                source: "okx".into(),
                namespace: crate::market::DEFAULT_NAMESPACE.into(),
                ingested_at: 0,
            });
        }
        out.sort_by_key(|b| b.ts);
        Ok(out)
    }
}

impl Default for OkxSource {
    fn default() -> Self {
        Self::new()
    }
}

/// Venues return numbers as strings (`"42123.5"`); accept either shape.
fn parse_num(v: &serde_json::Value) -> AppResult<f64> {
    match v {
        serde_json::Value::Number(n) => Ok(n.as_f64().unwrap_or(0.0)),
        serde_json::Value::String(s) => s
            .parse::<f64>()
            .map_err(|_| AppError::Source(format!("not a number: {s}"))),
        other => Err(AppError::Source(format!("not a number: {other}"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replay_is_deterministic() {
        let s = ReplaySource;
        let req = FetchRequest {
            symbol: "BTCUSDT".into(),
            interval: "1m".into(),
            from: 0,
            to: 300_000,
            limit: 100,
        };
        let a = s.fetch_bars(&req).unwrap();
        let b = s.fetch_bars(&req).unwrap();
        assert_eq!(a, b, "same request must produce identical bars");
        assert_eq!(a.len(), 6);
    }

    #[test]
    fn replay_bars_are_grid_aligned_and_ohlc_consistent() {
        let s = ReplaySource;
        let bars = s
            .fetch_bars(&FetchRequest {
                symbol: "ETHUSDT".into(),
                interval: "15m".into(),
                from: 61_000,
                to: 3_000_000,
                limit: 1000,
            })
            .unwrap();
        assert!(!bars.is_empty());
        for b in &bars {
            assert_eq!(b.ts % 900_000, 0, "ts must sit on the 15m grid");
            assert!(b.high >= b.open.max(b.close) - 1e-9, "high must cap open/close");
            assert!(b.low <= b.open.min(b.close) + 1e-9, "low must floor open/close");
            assert!(b.high >= b.low);
        }
    }

    #[test]
    fn replay_respects_the_limit() {
        let s = ReplaySource;
        let bars = s
            .fetch_bars(&FetchRequest {
                symbol: "BTCUSDT".into(),
                interval: "1m".into(),
                from: 0,
                to: 10_000_000,
                limit: 7,
            })
            .unwrap();
        assert_eq!(bars.len(), 7);
    }

    #[test]
    fn empty_range_yields_nothing() {
        let s = ReplaySource;
        let bars = s
            .fetch_bars(&FetchRequest {
                symbol: "BTCUSDT".into(),
                interval: "1m".into(),
                from: 5_000,
                to: 1_000,
                limit: 10,
            })
            .unwrap();
        assert!(bars.is_empty());
    }

    #[test]
    fn unknown_source_is_not_silently_substituted() {
        assert!(resolve("replay").is_some());
        assert!(resolve("binance").is_some());
        assert!(resolve("okx").is_some());
        assert!(resolve("tushare").is_none());
    }

    #[test]
    fn okx_symbol_and_interval_mapping() {
        assert_eq!(OkxSource::map_symbol("BTCUSDT"), "BTC-USDT");
        assert_eq!(OkxSource::map_interval("1h").unwrap(), "1H");
        assert_eq!(OkxSource::map_interval("1d").unwrap(), "1D");
        assert!(OkxSource::map_interval("2h").is_ok());
    }
}
