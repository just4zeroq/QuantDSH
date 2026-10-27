"""HTTP surface.

The route names and payload shapes intentionally mirror the Rust datahub that
this service replaces: the two shipped plugins (`data`, `market`) already speak
that contract, so swapping the backend must not require a frontend release.
New capabilities (source registry CRUD, batch audit) are additive.
"""

from __future__ import annotations

import csv
import hmac
import io
import json
import re
import time
from dataclasses import dataclass, field
from typing import Any

from fastapi import Body, Depends, FastAPI, Header, HTTPException, Query, Request
from fastapi.middleware.cors import CORSMiddleware

from .domain import (
    DEFAULT_NAMESPACE,
    DEFAULT_PRICE_TYPE,
    SUPPORTED_INTERVALS,
    dataset_key as build_dataset_key,
    instrument_id as build_instrument_id,
    interval_ms,
    is_interval,
    norm_symbol,
    now_ms,
    parse_dataset_key,
    parse_instrument_id,
)
from .providers.base import ProviderError, Unsupported
from .quality import check_bars
from .sync import BadRequest


@dataclass
class AppState:
    market: Any
    meta: Any
    registry: Any
    sync: Any
    token: str
    data_dir: str
    started_at: int = field(default_factory=now_ms)


_READ_ONLY = re.compile(
    r"\b(insert|update|delete|drop|create|alter|attach|detach|copy|export|import|install|load|pragma|call|vacuum|checkpoint|truncate)\b",
    re.IGNORECASE,
)


def _strip_sql(sql: str) -> str:
    """Remove string literals and comments before the read-only check.

    Without this, `SELECT '-- drop'` or a column named `update_ts` would trip
    the guard; with it, the check sees only executable tokens.
    """
    out = []
    i, n = 0, len(sql)
    while i < n:
        ch = sql[i]
        if ch == "'":
            i += 1
            while i < n:
                if sql[i] == "'" and i + 1 < n and sql[i + 1] == "'":
                    i += 2
                    continue
                if sql[i] == "'":
                    i += 1
                    break
                i += 1
            out.append(" ")
            continue
        if ch == "-" and i + 1 < n and sql[i + 1] == "-":
            while i < n and sql[i] != "\n":
                i += 1
            out.append(" ")
            continue
        if ch == "/" and i + 1 < n and sql[i + 1] == "*":
            i += 2
            while i + 1 < n and not (sql[i] == "*" and sql[i + 1] == "/"):
                i += 1
            i = min(i + 2, n)
            out.append(" ")
            continue
        out.append(ch)
        i += 1
    return "".join(out)


def create_app(state: AppState) -> FastAPI:
    app = FastAPI(title="QuantDSH datahub", version="2.0.0")
    app.add_middleware(
        CORSMiddleware,
        allow_origins=["*"],
        allow_methods=["*"],
        allow_headers=["*"],
    )
    app.state.hub = state

    def require_token(authorization: str | None = Header(default=None)) -> None:
        token = ""
        if authorization and authorization.lower().startswith("bearer "):
            token = authorization[7:].strip()
        if not token or not hmac.compare_digest(token, state.token):
            raise HTTPException(status_code=401, detail="unauthorized")

    # ── health (unauthenticated: the launcher probes it) ───────────────

    @app.get("/health")
    def health() -> dict[str, Any]:
        return {
            "status": "ok",
            "startedAt": state.started_at,
            "dataDir": state.data_dir,
            "engine": "python-fastapi",
            "sources": len(state.meta.list_sources()),
        }

    @app.on_event("shutdown")
    async def _shutdown() -> None:
        await state.registry.aclose()
        state.market.close()
        state.meta.close()

    # ── sources: the registry lives in SQLite ──────────────────────────

    @app.get("/api/sources", dependencies=[Depends(require_token)])
    def list_sources() -> dict[str, Any]:
        rows = state.meta.list_sources(include_disabled=False)
        return {
            # `sources` stays a plain string array for the shipped UI.
            "sources": [r["id"] for r in rows],
            "items": [_source_wire(r) for r in rows],
            "all": [_source_wire(r) for r in state.meta.list_sources()],
        }

    @app.post("/api/sources", dependencies=[Depends(require_token)])
    def upsert_source(payload: dict[str, Any] = Body(...)) -> dict[str, Any]:
        if not payload.get("id"):
            raise HTTPException(status_code=400, detail="id is required")
        try:
            row = state.meta.upsert_source(payload)
        except ValueError as exc:
            raise HTTPException(status_code=400, detail=str(exc)) from exc
        state.registry.invalidate(row["id"])
        return _source_wire(row)

    @app.delete("/api/sources/{source_id}", dependencies=[Depends(require_token)])
    def delete_source(source_id: str) -> dict[str, Any]:
        state.registry.invalidate(source_id)
        return {"deleted": state.meta.delete_source(source_id)}

    @app.post("/api/sources/{source_id}/probe", dependencies=[Depends(require_token)])
    async def probe_source(source_id: str) -> dict[str, Any]:
        try:
            return await state.sync.probe_source(source_id)
        except Unsupported as exc:
            raise HTTPException(status_code=400, detail=str(exc)) from exc
        except ProviderError as exc:
            raise HTTPException(status_code=502, detail=str(exc)) from exc

    @app.get("/api/instruments", dependencies=[Depends(require_token)])
    def list_instruments(
        venue: str | None = None,
        inst_type: str | None = Query(default=None, alias="type"),
        search: str | None = None,
    ) -> dict[str, Any]:
        rows = state.market.instruments(venue, inst_type)
        if search:
            needle = search.upper()
            rows = [r for r in rows if needle in (r["symbol"] or "").upper()]
        return {"instruments": rows}

    # ── datasets ───────────────────────────────────────────────────────

    @app.get("/api/datasets", dependencies=[Depends(require_token)])
    def list_datasets(market: str | None = None, namespace: str | None = None) -> list[dict[str, Any]]:
        return [_dataset_wire(r) for r in state.meta.list_datasets(market, namespace)]

    @app.post("/api/datasets", dependencies=[Depends(require_token)])
    def upsert_dataset(rec: dict[str, Any] = Body(...)) -> dict[str, Any]:
        if not rec.get("dataset_key"):
            raise HTTPException(status_code=400, detail="dataset_key is required")
        state.meta.upsert_dataset(rec)
        return {"ok": True}

    @app.post("/api/datasets/refresh", dependencies=[Depends(require_token)])
    def refresh_dataset(body: dict[str, Any] = Body(...)) -> dict[str, Any]:
        key = body.get("dataset_key")
        ds = state.meta.get_dataset(key) if key else None
        if not ds:
            raise HTTPException(status_code=404, detail="dataset not found")
        inst = ds.get("instrument_id") or build_instrument_id(
            ds.get("venue") or ds["exchange"], ds["symbol"], ds.get("inst_type") or "spot"
        )
        stats = state.market.stats(
            ds.get("namespace") or DEFAULT_NAMESPACE, inst, ds["interval"], ds.get("price_type") or DEFAULT_PRICE_TYPE
        )
        state.meta.upsert_dataset({**ds, "row_count": stats["rows"], "as_of": stats["to"], "ingested_at": now_ms()})
        return {"ok": True, **stats}

    @app.get("/api/datasets/summary", dependencies=[Depends(require_token)])
    def dataset_summary(dataset_key: str = Query(...)) -> dict[str, Any]:
        ds = state.meta.get_dataset(dataset_key)
        if not ds:
            raise HTTPException(status_code=404, detail="dataset not found")
        inst = ds.get("instrument_id") or build_instrument_id(
            ds.get("venue") or ds["exchange"], ds["symbol"], ds.get("inst_type") or "spot"
        )
        interval = ds["interval"]
        namespace = ds.get("namespace") or DEFAULT_NAMESPACE
        price_type = ds.get("price_type") or DEFAULT_PRICE_TYPE
        stats = state.market.stats(namespace, inst, interval, price_type)
        last = state.market.last_closed_ts(namespace, inst, interval, price_type)
        step = interval_ms(interval)
        cells = state.meta.coverage(dataset_key)
        present = sum(c["present"] for c in cells)
        expected = sum(c["expected"] for c in cells)
        return {
            "datasetKey": dataset_key,
            "instrumentId": inst,
            "rows": stats["rows"],
            "from": stats["from"],
            "to": stats["to"],
            "lastClosedTs": last,
            "freshnessMs": (now_ms() - last) if last else None,
            "intervalMs": step,
            "coverage": {"present": present, "expected": expected, "ratio": (present / expected) if expected else 0.0},
            "health": ds.get("health"),
        }

    # ── bars ───────────────────────────────────────────────────────────

    @app.get("/api/bars", dependencies=[Depends(require_token)])
    def get_bars(
        symbol: str | None = None,
        instrument_id: str | None = None,
        interval: str | None = None,
        namespace: str = DEFAULT_NAMESPACE,
        price_type: str = DEFAULT_PRICE_TYPE,
        from_ts: int | None = Query(default=None, alias="from"),
        to_ts: int | None = Query(default=None, alias="to"),
        limit: int = 5000,
    ) -> list[dict[str, Any]]:
        rows = state.market.bars(
            instrument_id=instrument_id,
            symbol=symbol,
            interval=interval,
            namespace=namespace,
            price_type=price_type,
            start=from_ts,
            end=to_ts,
            limit=limit,
        )
        return [_bar_wire(r) for r in rows]

    @app.get("/api/bars/timestamps", dependencies=[Depends(require_token)])
    def get_timestamps(
        symbol: str | None = None,
        instrument_id: str | None = None,
        interval: str | None = None,
        namespace: str = DEFAULT_NAMESPACE,
        from_ts: int | None = Query(default=None, alias="from"),
        to_ts: int | None = Query(default=None, alias="to"),
    ) -> dict[str, Any]:
        rows = state.market.timestamps(
            instrument_id=instrument_id,
            symbol=symbol,
            interval=interval,
            namespace=namespace,
            start=from_ts,
            end=to_ts,
        )
        return {"timestamps": rows}

    @app.delete("/api/bars", dependencies=[Depends(require_token)])
    def delete_bars(
        symbol: str,
        interval: str,
        namespace: str = DEFAULT_NAMESPACE,
        from_ts: int | None = Query(default=None, alias="from"),
        to_ts: int | None = Query(default=None, alias="to"),
        instrument_id: str | None = None,
    ) -> dict[str, Any]:
        inst = instrument_id or build_instrument_id("binance", symbol, "spot")
        before = state.market.stats(namespace, inst, interval)
        state.market.delete_bars(
            instrument_id=inst,
            interval=interval,
            namespace=namespace,
            start=from_ts,
            end=to_ts,
        )
        after = state.market.stats(namespace, inst, interval)
        return {"deleted": max(0, before["rows"] - after["rows"])}

    @app.post("/api/bars/correct", dependencies=[Depends(require_token)])
    def correct_bar(body: dict[str, Any] = Body(...)) -> dict[str, Any]:
        ts = body.get("ts")
        interval = body.get("interval")
        if ts is None or not interval:
            raise HTTPException(status_code=400, detail="ts and interval are required")
        inst = body.get("instrument_id") or build_instrument_id(
            body.get("venue") or "binance", body.get("symbol") or "BTC/USDT", body.get("inst_type") or "spot"
        )
        sets, params = [], []
        for col in ("open", "high", "low", "close", "volume", "turnover"):
            if col in body and body[col] is not None:
                sets.append(f"{col} = ?")
                params.append(float(body[col]))
        if not sets:
            return {"changed": False}
        params.extend([DEFAULT_NAMESPACE, inst, interval, ts])
        state.market.execute(
            f'UPDATE ohlcv SET {", ".join(sets)} WHERE namespace = ? AND instrument_id = ? AND "interval" = ? AND ts = ?',
            params,
        )
        return {"changed": True}

    # ── coverage ───────────────────────────────────────────────────────

    @app.get("/api/coverage", dependencies=[Depends(require_token)])
    def get_coverage(dataset_key: str = Query(...), month: str | None = None) -> dict[str, Any]:
        cells = state.meta.coverage(dataset_key, month)
        return {
            "cells": [
                {"day": c["day"], "hour": c["hour"], "present": c["present"], "expected": c["expected"]} for c in cells
            ]
        }

    @app.get("/api/coverage/summary", dependencies=[Depends(require_token)])
    def coverage_summary(dataset_key: str = Query(...)) -> dict[str, Any]:
        cells = state.meta.coverage(dataset_key)
        present = sum(c["present"] for c in cells)
        expected = sum(c["expected"] for c in cells)
        return {"present": present, "expected": expected, "ratio": (present / expected) if expected else 0.0}

    @app.post("/api/coverage/refresh", dependencies=[Depends(require_token)])
    def refresh_coverage(body: dict[str, Any] = Body(...)) -> dict[str, Any]:
        key = body["dataset_key"]
        ds = state.meta.get_dataset(key)
        if not ds:
            raise HTTPException(status_code=404, detail="dataset not found")
        inst = ds.get("instrument_id") or build_instrument_id(ds["exchange"], ds["symbol"], ds.get("inst_type") or "spot")
        year, month = int(body.get("year") or 1970), int(body.get("month") or 1)
        prefix = f"{year:04d}-{month:02d}"
        rows = state.market.query(
            "SELECT strftime(epoch_ms(ts), '%Y-%m-%d') AS day, CAST(strftime(epoch_ms(ts), '%H') AS INTEGER) AS hour, "
            'count(*) AS present FROM ohlcv WHERE namespace = ? AND instrument_id = ? AND "interval" = ? '
            "AND price_type = ? AND strftime(epoch_ms(ts), '%Y-%m') = ? GROUP BY 1, 2 ORDER BY 1, 2",
            [ds.get("namespace") or DEFAULT_NAMESPACE, inst, ds["interval"], ds.get("price_type") or DEFAULT_PRICE_TYPE, prefix],
        )
        expected = max(1, 3_600_000 // interval_ms(ds["interval"]))
        cells = [(r["day"], int(r["hour"]), int(r["present"]), expected) for r in rows]
        state.meta.replace_coverage(key, cells)
        return {
            "cells": [{"day": d, "hour": h, "present": p, "expected": e} for d, h, p, e in cells]
        }

    # ── jobs ───────────────────────────────────────────────────────────

    @app.get("/api/jobs", dependencies=[Depends(require_token)])
    def list_jobs(enabled_only: bool = False, dataset_key: str | None = None) -> list[dict[str, Any]]:
        return [_job_wire(j) for j in state.meta.list_jobs(enabled_only, dataset_key)]

    @app.post("/api/jobs", dependencies=[Depends(require_token)])
    def create_job(job: dict[str, Any] = Body(...)) -> dict[str, Any]:
        try:
            _validate_job(job, state)
        except (BadRequest, Unsupported, ValueError) as exc:
            raise HTTPException(status_code=400, detail=str(exc)) from exc
        job_id = state.meta.create_job(job)
        return {"id": job_id}

    @app.post("/api/jobs/{job_id}", dependencies=[Depends(require_token)])
    def update_job(job_id: str, patch: dict[str, Any] = Body(...)) -> dict[str, Any]:
        if state.meta.get_job(job_id) is None:
            raise HTTPException(status_code=404, detail="job not found")
        state.meta.update_job(job_id, patch)
        return {"ok": True}

    @app.delete("/api/jobs/{job_id}", dependencies=[Depends(require_token)])
    def delete_job(job_id: str) -> dict[str, Any]:
        return {"deleted": state.meta.delete_job(job_id)}

    @app.get("/api/jobs/due", dependencies=[Depends(require_token)])
    def due_jobs() -> list[dict[str, Any]]:
        # The poll scheduler is not part of v1; incremental runs are triggered
        # explicitly. Returning an empty list keeps the old client honest
        # instead of pretending a scheduler exists.
        return []

    @app.post("/api/jobs/{job_id}/run", dependencies=[Depends(require_token)])
    async def run_job(job_id: str) -> dict[str, Any]:
        if state.meta.get_job(job_id) is None:
            raise HTTPException(status_code=404, detail="job not found")
        try:
            outcome = await state.sync.run_job(job_id)
        except BadRequest as exc:
            raise HTTPException(status_code=400, detail=str(exc)) from exc
        except Unsupported as exc:
            raise HTTPException(status_code=400, detail=str(exc)) from exc
        except ProviderError as exc:
            raise HTTPException(status_code=502, detail=str(exc)) from exc
        job = state.meta.get_job(job_id) or {}
        return {
            "job_id": job_id,
            "status": outcome["status"],
            "rows_written": outcome["rowsWritten"],
            "rows_fetched": outcome["rowsFetched"],
            "cursor": job.get("cursor"),
            "from": job.get("from_ts"),
            "to": job.get("to_ts"),
            "batch_id": outcome["batchId"],
            "error": "; ".join(outcome["errors"][:5]) or None,
        }

    @app.get("/api/batches", dependencies=[Depends(require_token)])
    def list_batches(limit: int = 50, status: str | None = None) -> dict[str, Any]:
        return {"batches": state.meta.list_batches(limit, status)}

    # ── quality ────────────────────────────────────────────────────────

    @app.post("/api/quality", dependencies=[Depends(require_token)])
    def run_quality(body: dict[str, Any] = Body(default={})) -> dict[str, Any]:
        key = body.get("dataset_key")
        ds = state.meta.get_dataset(key) if key else None
        if not ds:
            symbol = body.get("symbol")
            interval = body.get("interval")
            ds = next(
                (d for d in state.meta.list_datasets() if d["symbol"] == symbol and d["interval"] == interval),
                None,
            )
        if not ds:
            raise HTTPException(status_code=404, detail="dataset not found")
        inst = ds.get("instrument_id") or build_instrument_id(ds["exchange"], ds["symbol"], ds.get("inst_type") or "spot")
        result = check_bars(
            state.market,
            instrument_id=inst,
            interval=ds["interval"],
            namespace=ds.get("namespace") or DEFAULT_NAMESPACE,
            price_type=ds.get("price_type") or DEFAULT_PRICE_TYPE,
        )
        run_id = f"q-{now_ms():x}"
        state.meta.save_quality_run(run_id, ds["dataset_key"], result["passed"], result["findings"])
        stats = state.market.stats(
            ds.get("namespace") or DEFAULT_NAMESPACE, inst, ds["interval"], ds.get("price_type") or DEFAULT_PRICE_TYPE
        )
        cells = state.meta.coverage(ds["dataset_key"])
        present = sum(c["present"] for c in cells)
        expected = sum(c["expected"] for c in cells)
        return {
            "findings": result["findings"],
            "passed": result["passed"],
            "expected": expected,
            "present": present,
            "coverage_ratio": (present / expected) if expected else 0.0,
            "min_ts": stats["from"],
            "max_ts": stats["to"],
        }

    @app.get("/api/quality/latest", dependencies=[Depends(require_token)])
    def latest_quality(dataset_key: str | None = None) -> dict[str, Any]:
        return {"run": state.meta.latest_quality(dataset_key)}

    # ── SQL ────────────────────────────────────────────────────────────

    @app.post("/api/query", dependencies=[Depends(require_token)])
    def run_query(body: dict[str, Any] = Body(...)) -> dict[str, Any]:
        sql = (body.get("sql") or "").strip().rstrip(";")
        max_rows = int(body.get("max_rows") or 1000)
        started = time.perf_counter()
        if not sql:
            return {"status": "blocked", "columns": [], "rows": [], "truncated": False, "duration_ms": 0, "reason": "empty sql"}
        if ";" in _strip_sql(sql):
            return {
                "status": "blocked",
                "columns": [],
                "rows": [],
                "truncated": False,
                "duration_ms": 0,
                "reason": "只允许单条语句",
            }
        if _READ_ONLY.search(_strip_sql(sql)):
            return {
                "status": "blocked",
                "columns": [],
                "rows": [],
                "truncated": False,
                "duration_ms": 0,
                "reason": "只允许只读查询",
            }
        try:
            cur = state.market.conn.execute(sql)
            columns = [d[0] for d in cur.description] if cur.description else []
            rows = cur.fetchmany(max_rows + 1)
            truncated = len(rows) > max_rows
            rows = rows[:max_rows]
            duration = int((time.perf_counter() - started) * 1000)
            state.meta.log_query(sql, "ok", len(rows), duration)
            return {
                "status": "ok",
                "columns": columns,
                "rows": [list(r) for r in rows],
                "truncated": truncated,
                "duration_ms": duration,
                "reason": None,
            }
        except Exception as exc:  # noqa: BLE001 - surfaced verbatim to the SQL tab
            duration = int((time.perf_counter() - started) * 1000)
            state.meta.log_query(sql, "error", 0, duration, str(exc))
            return {
                "status": "error",
                "columns": [],
                "rows": [],
                "truncated": False,
                "duration_ms": duration,
                "reason": str(exc),
            }

    @app.get("/api/query/history", dependencies=[Depends(require_token)])
    def query_history(limit: int = 30) -> dict[str, Any]:
        return {"history": state.meta.query_history(limit)}

    # ── misc ───────────────────────────────────────────────────────────

    @app.get("/api/markets", dependencies=[Depends(require_token)])
    def list_markets() -> dict[str, Any]:
        return {"markets": ["crypto"]}

    @app.get("/api/schema", dependencies=[Depends(require_token)])
    def schema() -> list[dict[str, Any]]:
        return state.market.schema()

    @app.get("/api/imports", dependencies=[Depends(require_token)])
    def list_imports(dataset_key: str | None = None, limit: int = 50) -> dict[str, Any]:
        rows = state.meta.list_imports(limit)
        if dataset_key:
            rows = [r for r in rows if r["dataset_key"] == dataset_key]
        return {"imports": rows}

    @app.post("/api/import/csv", dependencies=[Depends(require_token)])
    def import_csv(body: dict[str, Any] = Body(...)) -> dict[str, Any]:
        return _import_csv(state, body)

    @app.post("/api/import/promote", dependencies=[Depends(require_token)])
    def promote(body: dict[str, Any] = Body(...)) -> dict[str, Any]:
        key = body["dataset_key"]
        try:
            parsed = parse_dataset_key(key)
        except ValueError as exc:
            raise HTTPException(status_code=400, detail=str(exc)) from exc
        inst = build_instrument_id(parsed["venue"], parsed["symbol"], parsed["inst_type"])
        interval = parsed["interval"]
        moved = state.market.query(
            'SELECT count(*) AS n, min(ts) AS lo, max(ts) AS hi FROM ohlcv '
            'WHERE namespace = ? AND instrument_id = ? AND "interval" = ?',
            ["imported", inst, interval],
        )[0]
        state.market.execute(
            'INSERT INTO ohlcv SELECT instrument_id, "interval", price_type, ts, open, high, low, close, volume, '
            "turnover, trades_count, taker_buy_volume, vwap, closed, source, 'bronze', batch_id, ingested_at "
            'FROM ohlcv WHERE namespace = ? AND instrument_id = ? AND "interval" = ? '
            'ON CONFLICT (namespace, instrument_id, "interval", price_type, ts) DO NOTHING',
            ["imported", inst, interval],
        )
        stats = state.market.stats(DEFAULT_NAMESPACE, inst, interval)
        state.meta.upsert_dataset(
            {
                "dataset_key": build_dataset_key(parsed["venue"], parsed["symbol"], parsed["inst_type"], "ohlcv", interval),
                "market": parsed["market"],
                "exchange": parsed["venue"],
                "venue": parsed["venue"],
                "symbol": parsed["symbol"],
                "inst_type": parsed["inst_type"],
                "interval": interval,
                "data_type": "ohlcv",
                "instrument_id": inst,
                "source": body.get("source") or "csv",
                "namespace": DEFAULT_NAMESPACE,
                "as_of": stats["to"],
                "ingested_at": now_ms(),
                "row_count": stats["rows"],
                "mode": "backfill",
            }
        )
        return {"moved": int(moved["n"] or 0), "min_ts": moved["lo"], "max_ts": moved["hi"]}

    return app


# ─────────────────────────── helpers ───────────────────────────


def _bar_wire(row: dict[str, Any]) -> dict[str, Any]:
    """Map an `ohlcv` row back to the `Bar` shape the shipped chart expects."""
    try:
        symbol = parse_instrument_id(row["instrument_id"])["symbol"]
    except ValueError:
        symbol = row["instrument_id"]
    return {
        "symbol": symbol,
        "interval": row["interval"],
        "ts": row["ts"],
        "open": row["open"],
        "high": row["high"],
        "low": row["low"],
        "close": row["close"],
        "volume": row["volume"],
        "turnover": row["turnover"],
        "trades_count": row.get("trades_count"),
        "taker_buy_volume": row.get("taker_buy_volume"),
        "closed": bool(row["closed"]),
        "source": row["source"],
        "namespace": row["namespace"],
        "ingested_at": row["ingested_at"],
        "instrument_id": row["instrument_id"],
        "price_type": row["price_type"],
    }


def _source_wire(row: dict[str, Any]) -> dict[str, Any]:
    return {
        "id": row["id"],
        "label": row.get("label"),
        "provider": row.get("provider"),
        "venue": row.get("venue"),
        "kind": row.get("kind"),
        "baseUrls": row.get("base_urls") or [],
        "enabled": bool(row.get("enabled")),
        "priority": row.get("priority"),
        "instTypes": row.get("inst_types") or [],
        "intervals": row.get("intervals"),
        "authKind": row.get("auth_kind"),
        "authSecretRef": row.get("auth_secret_ref"),
        "config": row.get("config") or {},
        "capabilities": row.get("capabilities"),
        "capabilitiesAt": row.get("capabilities_at"),
        "status": row.get("status"),
        "statusAt": row.get("status_at"),
        "notes": row.get("notes") or [],
    }


def _dataset_wire(row: dict[str, Any]) -> dict[str, Any]:
    out = dict(row)
    out["closed_only"] = bool(row.get("closed_only", True))
    out["legacy_key"] = _legacy_key(row)
    return out


def _legacy_key(row: dict[str, Any]) -> str | None:
    """The 4-segment key the shipped UI still compares against."""
    try:
        return f"{row['market']}:{row['exchange']}:{row['symbol'].replace('/', '')}:{row['interval']}"
    except (KeyError, AttributeError):
        return None


def _job_wire(job: dict[str, Any]) -> dict[str, Any]:
    out = dict(job)
    out["symbols"] = job.get("symbols") or []
    out["enabled"] = bool(job.get("enabled"))
    return out


def _validate_job(job: dict[str, Any], state: AppState) -> None:
    source_id = job.get("source")
    if not source_id:
        raise BadRequest("source is required")
    source = state.meta.get_source(source_id)
    if not source:
        raise BadRequest(f"未知数据源: {source_id}")
    if not source.get("enabled"):
        raise BadRequest(f"数据源 {source_id} 已停用")
    interval = job.get("interval")
    if not is_interval(interval):
        raise BadRequest(f"interval/unsupported: {interval}")
    inst_type = job.get("inst_type") or "spot"
    if inst_type not in (source.get("inst_types") or []):
        raise BadRequest(f"{source_id} 不支持 {inst_type}")
    symbols = job.get("symbols") or []
    if not symbols:
        raise BadRequest("symbols 不能为空")
    for symbol in symbols:
        norm_symbol(symbol)
    if not job.get("dataset_key"):
        venue = job.get("venue") or source.get("venue")
        if not venue:
            raise BadRequest("聚合数据源需要在任务里指定 venue")
        job["dataset_key"] = build_dataset_key(venue, symbols[0], inst_type, "ohlcv", interval)


def _import_csv(state: AppState, body: dict[str, Any]) -> dict[str, Any]:
    text = body.get("text") or ""
    symbol = norm_symbol(body.get("symbol") or "")
    interval = body.get("interval")
    if not is_interval(interval):
        raise HTTPException(status_code=400, detail=f"interval/unsupported: {interval}")
    namespace = body.get("namespace") or "imported"
    key = body.get("dataset_key") or build_dataset_key(
        body.get("venue") or "binance", symbol, body.get("inst_type") or "spot", "ohlcv", interval
    )
    inst = build_instrument_id(body.get("venue") or "binance", symbol, body.get("inst_type") or "spot")

    reader = csv.DictReader(io.StringIO(text))
    if not reader.fieldnames:
        raise HTTPException(status_code=400, detail="CSV 缺少表头")
    headers = {h.strip().lower().lstrip("\ufeff"): h for h in reader.fieldnames if h}
    time_key = next((headers[k] for k in ("ts", "timestamp", "time", "datetime", "date", "open_time") if k in headers), None)
    if time_key is None:
        raise HTTPException(status_code=400, detail="CSV 缺少时间列（ts/timestamp/datetime）")

    def num(row: dict[str, Any], name: str) -> float | None:
        raw = row.get(headers.get(name, "")) if name in headers else None
        if raw in (None, ""):
            return None
        try:
            return float(raw)
        except ValueError:
            return None

    total = ok = 0
    bad_samples: list[str] = []
    bars = []
    from .domain import Bar, is_closed

    for row in reader:
        total += 1
        raw_ts = row.get(time_key) or ""
        try:
            ts = _parse_ts(raw_ts)
        except ValueError:
            if len(bad_samples) < 5:
                bad_samples.append(f"bad timestamp: {raw_ts}")
            continue
        o, h, l, c = num(row, "open"), num(row, "high"), num(row, "low"), num(row, "close")
        if None in (o, h, l, c):
            if len(bad_samples) < 5:
                bad_samples.append(f"missing ohlc at {raw_ts}")
            continue
        bar = Bar(
            symbol=symbol,
            interval=interval,
            ts=ts,
            open=o,
            high=h,
            low=l,
            close=c,
            volume=num(row, "volume"),
            turnover=num(row, "turnover"),
            closed=is_closed(ts, interval, now_ms()),
            source=body.get("source") or "csv",
            namespace=namespace,
            instrument_id=inst,
        )
        ok += 1
        bars.append(bar)

    written = state.market.upsert_bars(bars, namespace=namespace) if bars else 0
    tolerance = float(body.get("max_bad_ratio") or 0.01)
    passed = total > 0 and (total - ok) / total <= tolerance
    state.meta.log_import(
        {
            "id": f"imp-{now_ms():x}",
            "dataset_key": key,
            "namespace": namespace,
            "file": body.get("file") or "clipboard.csv",
            "rows_total": total,
            "rows_ok": ok,
            "rows_bad": total - ok,
            "gate_passed": passed,
            "gate_report": json.dumps({"bad_samples": bad_samples, "tolerance": tolerance}),
        }
    )
    stats = state.market.stats(namespace, inst, interval)
    state.meta.upsert_dataset(
        {
            "dataset_key": key,
            "market": "crypto",
            "exchange": body.get("venue") or "binance",
            "venue": body.get("venue") or "binance",
            "symbol": symbol,
            "inst_type": body.get("inst_type") or "spot",
            "interval": interval,
            "data_type": "ohlcv",
            "instrument_id": inst,
            "source": body.get("source") or "csv",
            "namespace": namespace,
            "as_of": stats["to"],
            "ingested_at": now_ms(),
            "row_count": stats["rows"],
            "mode": "backfill",
        }
    )
    return {
        "batch_id": f"imp-{now_ms():x}",
        "rows_total": total,
        "rows_ok": ok,
        "rows_bad": total - ok,
        "gate_passed": passed,
        "written": written,
        "findings": [],
        "bad_samples": bad_samples,
    }


def _parse_ts(raw: str) -> int:
    value = str(raw).strip()
    if not value:
        raise ValueError("empty")
    if re.fullmatch(r"\d+", value):
        number = int(value)
        # 10 digits = seconds, 13 = milliseconds. Anything else is treated as
        # milliseconds, which is what every venue API returns.
        return number * 1000 if number < 100_000_000_000 else number
    for fmt in ("%Y-%m-%dT%H:%M:%S", "%Y-%m-%d %H:%M:%S", "%Y-%m-%d", "%Y/%m/%d %H:%M:%S"):
        try:
            import datetime as _dt

            return int(_dt.datetime.strptime(value.replace("Z", ""), fmt).replace(tzinfo=_dt.timezone.utc).timestamp() * 1000)
        except ValueError:
            continue
    raise ValueError(f"bad timestamp: {raw}")
