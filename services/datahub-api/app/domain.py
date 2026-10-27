"""Shared domain vocabulary: intervals, instruments, dataset keys, bars.

One rule holds the whole design together: the **venue** (which exchange the
data belongs to) is part of the instrument id, while the **source** (which
channel we fetched it through) is only recorded as provenance. So a bar pulled
from Binance directly and the same bar pulled through our own aggregation API
land on the same primary key — switching channels can never create a second
copy of the same market.
"""

from __future__ import annotations

import re
from dataclasses import dataclass, field
from typing import Any

INTERVAL_MS: dict[str, int] = {
    "1m": 60_000,
    "3m": 180_000,
    "5m": 300_000,
    "15m": 900_000,
    "30m": 1_800_000,
    "1h": 3_600_000,
    "2h": 7_200_000,
    "4h": 14_400_000,
    "6h": 21_600_000,
    "12h": 43_200_000,
    "1d": 86_400_000,
    "3d": 259_200_000,
    "1w": 604_800_000,
}

# Intervals the UI offers. Anything else is rejected before a request is made.
SUPPORTED_INTERVALS = ["1m", "5m", "15m", "30m", "1h", "4h", "1d"]

INST_TYPES = ["spot", "swap"]
PRICE_TYPES = ["trade", "mark", "index", "premium"]

DEFAULT_NAMESPACE = "bronze"
DEFAULT_PRICE_TYPE = "trade"


def interval_ms(interval: str) -> int:
    try:
        return INTERVAL_MS[interval]
    except KeyError:
        raise ValueError(f"interval/unsupported: {interval}") from None


def is_interval(interval: str) -> bool:
    return interval in INTERVAL_MS


def floor_to(ts: int, interval: str) -> int:
    step = interval_ms(interval)
    return (ts // step) * step


def is_closed(ts: int, interval: str, now_ms: int, skew_ms: int = 500) -> bool:
    """A bar is final only once its close time has passed (minus clock skew)."""
    return ts + interval_ms(interval) <= now_ms - skew_ms


_SYMBOL_RE = re.compile(r"^[A-Z0-9]+/[A-Z0-9]+$")


def norm_symbol(symbol: str) -> str:
    """Accept `BTCUSDT`, `BTC-USDT`, `BTC/USDT`, `btc/usdt` → `BTC/USDT`.

    Venue-specific spellings are produced by the providers, never stored. The
    database only ever holds the unified form, which is what makes an
    instrument comparable across exchanges.

    Deliberately permissive about the characters in base/quote: venues list
    tokens whose ticker is not `[A-Z0-9]+` (meme coins with CJK names, `1000SATS`,
    `BTC.d`). Rejecting them would silently drop a whole venue's instruments.
    """
    raw = symbol.strip().upper().replace("-", "/").replace("_", "/")
    if "/" in raw:
        base, quote = raw.split("/", 1)
        if base and quote and "/" not in quote:
            return f"{base}/{quote}"
        raise ValueError(f"symbol/unsupported: {symbol}")
    for quote in ("USDT", "USDC", "USD", "BTC", "ETH", "EUR"):
        if raw.endswith(quote) and len(raw) > len(quote):
            return f"{raw[: -len(quote)]}/{quote}"
    raise ValueError(f"symbol/unsupported: {symbol}")


def instrument_id(venue: str, symbol: str, inst_type: str = "spot") -> str:
    """`crypto:binance:BTC/USDT:swap` — venue, not source."""
    if inst_type not in INST_TYPES:
        raise ValueError(f"inst_type/unsupported: {inst_type}")
    return f"crypto:{venue}:{norm_symbol(symbol)}:{inst_type}"


def parse_instrument_id(instrument: str) -> dict[str, str]:
    parts = instrument.split(":")
    if len(parts) != 4:
        raise ValueError(f"instrument_id/malformed: {instrument}")
    market, venue, symbol, inst_type = parts
    return {"market": market, "venue": venue, "symbol": symbol, "inst_type": inst_type}


def dataset_key(
    venue: str,
    symbol: str,
    inst_type: str = "spot",
    data_type: str = "ohlcv",
    interval: str = "1m",
    price_type: str = DEFAULT_PRICE_TYPE,
) -> str:
    """`crypto:binance:BTC/USDT:swap:ohlcv:1m` (6 segments).

    The price type rides on the interval segment (`1m.mark`) so the key stays
    6 segments while mark/index prices can never overwrite traded prices.
    """
    iv = interval if price_type == DEFAULT_PRICE_TYPE else f"{interval}.{price_type}"
    return f"{instrument_id(venue, symbol, inst_type)}:{data_type}:{iv}"


def split_interval(segment: str) -> tuple[str, str]:
    if "." in segment:
        interval, price_type = segment.split(".", 1)
        return interval, price_type
    return segment, DEFAULT_PRICE_TYPE


_LEGACY_RE = re.compile(r"^(?P<market>[a-z]+):(?P<venue>[a-z0-9]+):(?P<symbol>[^:]+):(?P<interval>[^:]+)$")


def parse_dataset_key(key: str) -> dict[str, str]:
    """Parse both the 6-segment form and the legacy 4-segment form.

    Legacy keys (`crypto:binance:BTCUSDT:1m`) predate the spot/swap dimension;
    they are read as spot, which is what the only writer at the time produced.
    """
    parts = key.split(":")
    if len(parts) == 6:
        market, venue, symbol, inst_type, data_type, segment = parts
        interval, price_type = split_interval(segment)
        return {
            "market": market,
            "venue": venue,
            "symbol": symbol,
            "inst_type": inst_type,
            "data_type": data_type,
            "interval": interval,
            "price_type": price_type,
        }
    match = _LEGACY_RE.match(key)
    if not match:
        raise ValueError(f"dataset_key/malformed: {key}")
    interval, price_type = split_interval(match.group("interval"))
    return {
        "market": match.group("market"),
        "venue": match.group("venue"),
        "symbol": norm_symbol(match.group("symbol")),
        "inst_type": "spot",
        "data_type": "ohlcv",
        "interval": interval,
        "price_type": price_type,
    }


def now_ms() -> int:
    import time

    return int(time.time() * 1000)


@dataclass
class Bar:
    """One OHLCV row in unified form.

    `volume` is always expressed in **base units** (coins). Venues that report
    contracts are converted by the provider using the market's contract size —
    that is the only reason `instruments.contract_size` exists.
    """

    symbol: str
    interval: str
    ts: int
    open: float
    high: float
    low: float
    close: float
    volume: float | None = None
    turnover: float | None = None
    trades_count: int | None = None
    taker_buy_volume: float | None = None
    closed: bool = True
    source: str = "unknown"
    namespace: str = DEFAULT_NAMESPACE
    ingested_at: int = 0
    instrument_id: str = ""
    price_type: str = DEFAULT_PRICE_TYPE

    def as_api(self) -> dict[str, Any]:
        """The wire shape. Field names match the Rust service exactly so an
        already-shipped frontend keeps working after the backend swap."""
        return {
            "symbol": self.symbol,
            "interval": self.interval,
            "ts": self.ts,
            "open": self.open,
            "high": self.high,
            "low": self.low,
            "close": self.close,
            "volume": self.volume,
            "turnover": self.turnover,
            "tradesCount": self.trades_count,
            "takerBuyVolume": self.taker_buy_volume,
            "closed": self.closed,
            "source": self.source,
            "namespace": self.namespace,
            "ingested_at": self.ingested_at,
            "instrumentId": self.instrument_id,
            "priceType": self.price_type,
        }


@dataclass
class Capabilities:
    """What one (venue, inst_type) pair can actually do over REST."""

    intervals: list[str] = field(default_factory=lambda: list(SUPPORTED_INTERVALS))
    max_page: int = 1000
    order: str = "asc"  # asc | desc — decides the pagination direction
    history_depth_ms: int | None = None
    min_poll_interval_ms: int = 60_000
    notes: list[str] = field(default_factory=list)

    def as_api(self) -> dict[str, Any]:
        return {
            "intervals": self.intervals,
            "maxPage": self.max_page,
            "order": self.order,
            "historyDepthMs": self.history_depth_ms,
            "minPollIntervalMs": self.min_poll_interval_ms,
            "notes": self.notes,
        }
