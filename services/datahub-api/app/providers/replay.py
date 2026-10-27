"""Deterministic offline source.

Same input always produces the same bars, which is what makes the whole
fetch → write → catalog → coverage → quality path testable without a network,
and lets a user click through the workbench on a plane. It is a first-class
`source` row (`replay`), not a test hook, so the UI exercises the real code
path.
"""

from __future__ import annotations

from ..domain import Bar, Capabilities, interval_ms, is_closed, now_ms
from .base import OhlcvRequest, Provider, Unsupported

_MASK = (1 << 64) - 1


def _mix(x: int) -> int:
    x = (x + 0x9E3779B97F4A7C15) & _MASK
    x ^= x >> 30
    x = (x * 0xBF58476D1CE4E5B9) & _MASK
    x ^= x >> 27
    x = (x * 0x94D049BB133111EB) & _MASK
    return x ^ (x >> 31)


def _hash_str(s: str) -> int:
    h = 0xCBF29CE484222325
    for b in s.encode("utf-8"):
        h ^= b
        h = (h * 0x100000001B3) & _MASK
    return _mix(h)


def _unit(symbol: str, ts: int, channel: int) -> float:
    h = _mix(_hash_str(symbol) ^ ((ts & _MASK) * 0x9E3779B9) ^ (channel << 17))
    return (h >> 11) / float(1 << 53)


class ReplayProvider(Provider):
    kind = "replay"

    async def capabilities(self, inst_type: str = "spot") -> Capabilities:
        return Capabilities(
            max_page=1000,
            order="asc",
            history_depth_ms=None,
            min_poll_interval_ms=60_000,
            notes=["离线自检源：不联网，用于验证入库与质检链路"],
        )

    async def instruments(self, inst_type: str = "spot") -> list[dict]:
        if inst_type != "spot":
            raise Unsupported("replay 只提供现货")
        out = []
        for sym in ("BTC/USDT", "ETH/USDT"):
            out.append(
                {
                    "instrument_id": f"crypto:replay:{sym}:spot",
                    "venue": "replay",
                    "symbol": sym,
                    "venue_symbol": sym.replace("/", ""),
                    "base": sym.split("/")[0],
                    "quote": sym.split("/")[1],
                    "inst_type": "spot",
                    "contract_size": 1.0,
                    "tick_size": 0.01,
                    "lot_size": 0.00001,
                    "active": True,
                }
            )
        return out

    async def fetch_ohlcv(self, req: OhlcvRequest) -> list[Bar]:
        if req.inst_type != "spot":
            raise Unsupported("replay 只提供现货")
        step = interval_ms(req.interval)
        if req.until < req.since:
            return []
        # Round the start **up** to the grid: a provider must never return a bar
        # older than `since`, otherwise a backfill writes one row outside the
        # window the caller asked for (and coverage counts it in the wrong day).
        first = -(-req.since // step) * step
        last = (req.until // step) * step
        base_price = 50.0 + (_hash_str(req.symbol) % 900)
        now = now_ms()

        out: list[Bar] = []
        ts = first
        while ts <= last and len(out) < req.limit:
            open_ = base_price * (1.0 + _unit(req.symbol, ts, 1) * 0.02)
            close = open_ * (1.0 + (_unit(req.symbol, ts, 2) - 0.5) * 0.01)
            hi, lo = max(open_, close), min(open_, close)
            high = hi * (1.0 + _unit(req.symbol, ts, 3) * 0.005)
            low = lo * (1.0 - _unit(req.symbol, ts, 4) * 0.005)
            volume = 100.0 + _unit(req.symbol, ts, 5) * 900.0
            out.append(
                Bar(
                    symbol=req.symbol,
                    interval=req.interval,
                    ts=ts,
                    open=round(open_, 6),
                    high=round(high, 6),
                    low=round(low, 6),
                    close=round(close, 6),
                    volume=round(volume, 6),
                    turnover=round(volume * close, 6),
                    closed=is_closed(ts, req.interval, now),
                    source=f"{self.id}:replay",
                    instrument_id=f"crypto:replay:{req.symbol}:spot",
                )
            )
            ts += step
        return out
