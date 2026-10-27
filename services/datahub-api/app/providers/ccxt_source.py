"""Exchange adapter backed by ccxt (REST only).

Why ccxt instead of hand-written adapters: it already encodes each venue's
symbol spelling, paging limits, rate limits and payload quirks. We deliberately
use only the **request/response** half of it — no `watch*` methods — because the
data layer's product is a replayable historical table, not a live stream.

ccxt's synchronous client is used on purpose: its async client binds sessions to
an event loop, which fights with a long-lived FastAPI process that restarts its
loop in tests. Calls are pushed to a worker thread instead.
"""

from __future__ import annotations

import threading
import time
from typing import Any

import anyio
import ccxt

from ..domain import SUPPORTED_INTERVALS, Bar, Capabilities, instrument_id, is_closed, now_ms
from .base import OhlcvRequest, Provider, ProviderError, SourceConfig, Unsupported

# ccxt reports contract size only for derivative markets; spot must stay at 1.
_DEFAULT_SETTLE = "USDT"


class CcxtProvider(Provider):
    kind = "ccxt"

    def __init__(self, cfg: SourceConfig) -> None:
        super().__init__(cfg)
        self._exchange: ccxt.Exchange | None = None
        self._markets: dict[str, Any] = {}
        self._lock = threading.Lock()
        self._notes: list[str] = []
        self._active_override: str | None = None
        self._inst_type_loaded: str | None = None

    # ── exchange lifecycle ──────────────────────────────────────────────

    @property
    def venue(self) -> str:
        if not self.cfg.venue:
            raise ProviderError(f"source/{self.id}: ccxt provider requires a venue")
        return self.cfg.venue

    def _candidates(self) -> list[dict[str, str] | None]:
        """URL overrides to try in order.

        A venue may answer on one domain and time out on another (this machine
        cannot reach `api.binance.com` but `data-api.binance.vision` serves the
        same public endpoints). When `url_overrides` is configured it is the
        whole list — the operator's order is authoritative; otherwise ccxt's
        own default is the only candidate.
        """
        raw = self.cfg.config.get("url_overrides")
        if not isinstance(raw, list) or not raw:
            return [None]
        return [c for c in raw if isinstance(c, dict)] or [None]

    def _build(self, inst_type: str, override: dict[str, str] | None) -> ccxt.Exchange:
        klass = getattr(ccxt, self.venue, None)
        if klass is None:
            raise Unsupported(f"ccxt 不认识交易所 {self.venue}")
        # `fetchMarkets` must be narrowed: Binance's loadMarkets otherwise pulls
        # spot *and* USDⓈ-M *and* COIN-M in one go, so a spot-only task fails
        # when the futures host is unreachable — and vice versa.
        options = {
            "defaultType": "swap" if inst_type == "swap" else "spot",
            "fetchMarkets": ["swap"] if inst_type == "swap" else ["spot"],
        }
        options.update(self.cfg.config.get("ccxt_options") or {})
        ex = klass(
            {
                "enableRateLimit": True,
                "timeout": int(self.cfg.config.get("timeout_ms") or 15_000),
                "options": options,
            }
        )
        if override:
            api = ex.urls.get("api")
            if isinstance(api, dict):
                for key, value in override.items():
                    if key in api:
                        api[key] = value
            elif isinstance(api, str) and "default" in override:
                ex.urls["api"] = override["default"]
        if self.cfg.config.get("proxy"):
            ex.proxies = {"http": self.cfg.config["proxy"], "https": self.cfg.config["proxy"]}
            ex.https_proxy = self.cfg.config["proxy"]
        return ex

    def _ensure(self, inst_type: str) -> ccxt.Exchange:
        with self._lock:
            if self._exchange is not None and self._inst_type_loaded == inst_type:
                return self._exchange
            if self._exchange is not None:
                try:
                    self._exchange.close()
                except Exception:  # noqa: BLE001 - closing is best effort
                    pass
                self._exchange = None
                self._markets = {}

            errors: list[str] = []
            for override in self._candidates():
                ex = self._build(inst_type, override)
                try:
                    ex.load_markets()
                    self._exchange = ex
                    self._markets = ex.markets or {}
                    self._inst_type_loaded = inst_type
                    if override:
                        self._notes = [f"已切换到备用域名 {override}"]
                    elif errors:
                        self._notes = [f"主域不可用，回退默认域: {errors[-1]}"]
                    else:
                        self._notes = []
                    return ex
                except ccxt.BaseError as exc:  # noqa: PERF203 - sequential fallback
                    errors.append(f"{type(exc).__name__}: {exc}")
                    try:
                        ex.close()
                    except Exception:  # noqa: BLE001
                        pass
            raise ProviderError(f"{self.venue}/{inst_type} 不可达: {errors[-1] if errors else 'unknown'}")

    # ── symbol mapping ─────────────────────────────────────────────────

    def _ccxt_symbol(self, req: OhlcvRequest) -> str:
        if req.inst_type == "spot":
            return req.symbol
        settle = req.settle or _DEFAULT_SETTLE
        return f"{req.symbol}:{settle}"

    def _contract_size(self, symbol: str) -> float:
        market = self._markets.get(symbol) or {}
        return float(market.get("contractSize") or 1.0)

    # ── Provider API ───────────────────────────────────────────────────

    async def capabilities(self, inst_type: str = "spot") -> Capabilities:
        if inst_type not in self.cfg.inst_types:
            raise Unsupported(f"{self.venue} 源未启用 {inst_type}")

        def probe() -> Capabilities:
            ex = self._ensure(inst_type)
            native = ex.timeframes or {}
            intervals = [i for i in SUPPORTED_INTERVALS if i in native] or list(SUPPORTED_INTERVALS)
            page = 1000 if inst_type == "spot" else 1500
            if self.venue == "okx":
                page = 300
            elif self.venue == "bybit":
                page = 1000
            return Capabilities(
                intervals=intervals,
                max_page=page,
                order="asc",
                history_depth_ms=None,
                min_poll_interval_ms=60_000,
                notes=list(self._notes),
            )

        return await anyio.to_thread.run_sync(probe)

    async def instruments(self, inst_type: str = "spot") -> list[dict[str, Any]]:
        def load() -> list[dict[str, Any]]:
            ex = self._ensure(inst_type)
            out: list[dict[str, Any]] = []
            for market in self._markets.values():
                if not market.get("active", True):
                    continue
                if inst_type == "spot" and not market.get("spot"):
                    continue
                if inst_type == "swap" and not market.get("swap"):
                    continue
                symbol = market.get("symbol")
                base, quote = market.get("base"), market.get("quote")
                if not symbol or not base or not quote:
                    continue
                latest = float(self._contract_size(symbol))
                price_precision = float(market.get("precision", {}).get("price") or 0)
                out.append(
                    {
                        "instrument_id": instrument_id(self.venue, f"{base}/{quote}", inst_type),
                        "venue": self.venue,
                        "symbol": f"{base}/{quote}",
                        "venue_symbol": market.get("id") or symbol,
                        "base": base,
                        "quote": quote,
                        "settle": market.get("settle"),
                        "inst_type": inst_type,
                        "contract_size": latest,
                        "tick_size": price_precision or None,
                        "active": True,
                        "updated_at": now_ms(),
                    }
                )
            ex  # keep the reference explicit for readers
            return out

        return await anyio.to_thread.run_sync(load)

    async def fetch_ohlcv(self, req: OhlcvRequest) -> list[Bar]:
        if req.interval not in SUPPORTED_INTERVALS:
            raise Unsupported(f"interval/unsupported: {req.interval}")
        if req.inst_type not in self.cfg.inst_types:
            raise Unsupported(f"{self.venue} 源未启用 {req.inst_type}")

        def fetch() -> list[Bar]:
            self._ensure(req.inst_type)
            symbol = self._ccxt_symbol(req)
            try:
                raw = self._exchange.fetch_ohlcv(  # type: ignore[union-attr]
                    symbol,
                    timeframe=req.interval,
                    since=req.since,
                    limit=max(1, min(req.limit, 1500)),
                )
            except ccxt.BadSymbol as exc:
                raise Unsupported(f"{self.venue} 无此标的 {symbol}") from exc
            except ccxt.BaseError as exc:
                raise ProviderError(f"{self.venue} {req.interval} 请求失败: {exc}") from exc

            cs = self._contract_size(symbol)
            # ccxt's unified ohlcv drops quote volume and trade count, so
            # `turnover` stays NULL rather than being faked as volume*close.
            now = now_ms()
            out: list[Bar] = []
            for row in raw:
                if len(row) < 6:
                    continue
                ts = int(row[0])
                if ts < req.since or ts > req.until:
                    continue
                out.append(
                    Bar(
                        symbol=req.symbol,
                        interval=req.interval,
                        ts=ts,
                        open=float(row[1]),
                        high=float(row[2]),
                        low=float(row[3]),
                        close=float(row[4]),
                        volume=float(row[5]) * cs,
                        turnover=None,
                        closed=is_closed(ts, req.interval, now),
                        source=f"{self.id}:ccxt",
                        instrument_id=instrument_id(self.venue, req.symbol, req.inst_type),
                    )
                )
            out.sort(key=lambda b: b.ts)
            return out

        return await anyio.to_thread.run_sync(fetch)

    async def self_check(self) -> dict[str, Any]:
        def check() -> dict[str, Any]:
            started = time.perf_counter()
            try:
                self._ensure("spot")
                self._exchange.fetch_time()  # type: ignore[union-attr]
                return {
                    "ok": True,
                    "latencyMs": int((time.perf_counter() - started) * 1000),
                    "notes": list(self._notes),
                }
            except Exception as exc:  # noqa: BLE001 - surfaced to the UI
                return {"ok": False, "error": str(exc), "notes": list(self._notes)}

        return await anyio.to_thread.run_sync(check)

    async def aclose(self) -> None:
        if self._exchange is not None:
            self._exchange.close()
            self._exchange = None
