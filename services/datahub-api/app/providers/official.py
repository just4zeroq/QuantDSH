"""Our own aggregation API as a data source.

This provider exists because the same market can be reached through more than
one channel: directly at the exchange (ccxt) or through our server-side
collector. The collector is not a different exchange — it republishes Binance,
OKX and Bybit data in one normalized shape — so a bar fetched here carries the
**same** `instrument_id` as the direct fetch and lands on the same primary key.
Choosing a channel changes provenance, never the dataset.

Wire contract (v1, served by the collector):

    GET {base}/api/v1/venues
        -> {"venues":[{"venue":"binance","label":"币安",
                        "instTypes":["spot","swap"],
                        "intervals":["1m","5m","15m","1h","4h","1d"]}]}

    GET {base}/api/v1/instruments?venue=binance&instType=swap
        -> {"instruments":[{"symbol":"BTC/USDT","venueSymbol":"BTCUSDT",
                            "base":"BTC","quote":"USDT","settle":"USDT",
                            "contractSize":1}]}

    GET {base}/api/v1/ohlcv?venue=binance&symbol=BTC/USDT&instType=swap
            &interval=1m&from=<ms>&to=<ms>&limit=1000
        -> {"bars":[{"ts":..., "open":..., "high":..., "low":..., "close":...,
                     "volume":..., "turnover":..., "tradesCount":...,
                     "closed":true}],
            "next": <ms|null>}

`volume` must be in base units and `next` must be null on the last page. Both
are part of the contract because they cannot be derived locally: contract size
is venue knowledge, and guessing the end of a page would silently truncate a
backfill.
"""

from __future__ import annotations

import os
from typing import Any

import httpx

from ..domain import Bar, Capabilities, instrument_id, is_closed, now_ms
from .base import OhlcvRequest, Provider, ProviderError, Unsupported


class OfficialProvider(Provider):
    kind = "official"

    def __init__(self, cfg) -> None:
        super().__init__(cfg)
        self._client: httpx.AsyncClient | None = None
        self._bases = [b.rstrip("/") for b in (cfg.base_urls or []) if b]
        self._active: str | None = None

    @property
    def bases(self) -> list[str]:
        if not self._bases:
            raise Unsupported("官方 API 源未配置 base_urls")
        return self._bases

    def _headers(self) -> dict[str, str]:
        if self.cfg.auth_kind in ("none", "", None):
            return {}
        secret = None
        if self.cfg.auth_secret_ref:
            secret = os.environ.get(self.cfg.auth_secret_ref)
        if not secret:
            secret = self.cfg.config.get("api_key")
        if not secret:
            raise Unsupported(
                f"官方 API 需要鉴权：请设置环境变量 {self.cfg.auth_secret_ref or 'QUANTDSH_OFFICIAL_TOKEN'}"
            )
        if self.cfg.auth_kind == "header":
            name = self.cfg.config.get("auth_header") or "X-API-Key"
            return {name: secret}
        return {"Authorization": f"Bearer {secret}"}

    async def _get(self, path: str, params: dict[str, Any] | None = None) -> dict[str, Any]:
        if self._client is None:
            self._client = httpx.AsyncClient(
                timeout=float(self.cfg.config.get("timeout_ms") or 20_000) / 1000.0
            )
        errors: list[str] = []
        candidates = [self._active] if self._active else self.bases
        for base in candidates:
            if not base:
                continue
            try:
                resp = await self._client.get(f"{base}{path}", params=params, headers=self._headers())
                if resp.status_code >= 400:
                    # A 4xx is the server's real answer; never fail over on it.
                    raise ProviderError(f"官方 API {resp.status_code}: {resp.text[:200]}")
                self._active = base
                return resp.json()
            except (httpx.TransportError, httpx.TimeoutException) as exc:
                errors.append(f"{base}: {exc}")
                self._active = None
        raise ProviderError(f"官方 API 不可达: {'; '.join(errors) or 'no base url'}")

    async def capabilities(self, inst_type: str = "spot") -> Capabilities:
        data = await self._get("/api/v1/venues")
        venues = data.get("venues") or []
        match = next((v for v in venues if inst_type in (v.get("instTypes") or [])), None)
        if match is None and venues:
            # The collector advertises everything it has; the caller decides by
            # venue, so a missing inst_type here means "not collected yet".
            raise Unsupported(f"官方 API 未提供 {inst_type} 数据")
        return Capabilities(
            intervals=list((match or {}).get("intervals") or []),
            max_page=int((match or {}).get("maxPage") or 1000),
            order="asc",
            history_depth_ms=(match or {}).get("historyDepthMs"),
            min_poll_interval_ms=int((match or {}).get("minPollIntervalMs") or 60_000),
            notes=[f"通道：官方聚合 API {self._active or ''}".strip()],
        )

    async def instruments(self, inst_type: str = "spot") -> list[dict[str, Any]]:
        data = await self._get("/api/v1/instruments", {"instType": inst_type})
        out: list[dict[str, Any]] = []
        for row in data.get("instruments") or []:
            venue = row.get("venue") or self.cfg.venue
            if not venue:
                continue
            symbol = row.get("symbol")
            if not symbol:
                continue
            out.append(
                {
                    "instrument_id": instrument_id(venue, symbol, inst_type),
                    "venue": venue,
                    "symbol": symbol,
                    "venue_symbol": row.get("venueSymbol") or symbol.replace("/", ""),
                    "base": row.get("base"),
                    "quote": row.get("quote"),
                    "settle": row.get("settle"),
                    "inst_type": inst_type,
                    "contract_size": float(row.get("contractSize") or 1.0),
                    "tick_size": row.get("tickSize"),
                    "active": bool(row.get("active", True)),
                    "updated_at": now_ms(),
                }
            )
        return out

    async def fetch_ohlcv(self, req: OhlcvRequest) -> list[Bar]:
        data = await self._get(
            "/api/v1/ohlcv",
            {
                "venue": req.venue,
                "symbol": req.symbol,
                "instType": req.inst_type,
                "interval": req.interval,
                "from": req.since,
                "to": req.until,
                "limit": req.limit,
            },
        )
        now = now_ms()
        inst = instrument_id(req.venue, req.symbol, req.inst_type)
        out: list[Bar] = []
        for row in data.get("bars") or []:
            ts = int(row["ts"])
            if ts < req.since or ts > req.until:
                continue
            out.append(
                Bar(
                    symbol=req.symbol,
                    interval=req.interval,
                    ts=ts,
                    open=float(row["open"]),
                    high=float(row["high"]),
                    low=float(row["low"]),
                    close=float(row["close"]),
                    volume=_maybe_float(row.get("volume")),
                    turnover=_maybe_float(row.get("turnover")),
                    trades_count=row.get("tradesCount"),
                    taker_buy_volume=_maybe_float(row.get("takerBuyVolume")),
                    closed=bool(row.get("closed", is_closed(ts, req.interval, now))),
                    source=f"{self.id}:official",
                    instrument_id=inst,
                )
            )
        out.sort(key=lambda b: b.ts)
        return out

    async def self_check(self) -> dict[str, Any]:
        try:
            data = await self._get("/api/v1/venues")
            venues = [v.get("venue") for v in (data.get("venues") or [])]
            return {"ok": True, "notes": [f"官方 API 提供 {len(venues)} 个交易所: {', '.join(filter(None, venues))}"]}
        except Exception as exc:  # noqa: BLE001 - surfaced to the UI
            return {"ok": False, "error": str(exc)}

    async def aclose(self) -> None:
        if self._client is not None:
            await self._client.aclose()
            self._client = None


def _maybe_float(value: Any) -> float | None:
    if value is None:
        return None
    try:
        return float(value)
    except (TypeError, ValueError):
        return None
