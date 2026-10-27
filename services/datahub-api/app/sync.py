"""Sync engine: one job in, bars on disk, catalog + coverage + quality updated.

Every job run is serialized (`asyncio.Lock`): DuckDB has a single writer, so
running two backfills in parallel would only make them both slower and produce
interleaved batches that are impossible to audit.
"""

from __future__ import annotations

import asyncio
from typing import Any

from .domain import (
    DEFAULT_NAMESPACE,
    DEFAULT_PRICE_TYPE,
    Bar,
    dataset_key,
    instrument_id,
    interval_ms,
    is_interval,
    norm_symbol,
    now_ms,
)
from .providers.base import OhlcvRequest, ProviderError, Unsupported
from .quality import check_bars, duplicates


class BadRequest(ValueError):
    pass


class SyncService:
    def __init__(self, market, meta, registry) -> None:
        self._market = market
        self._meta = meta
        self._registry = registry
        self._lock = asyncio.Lock()

    # ── public ─────────────────────────────────────────────────────────

    async def run_job(self, job_id: str) -> dict[str, Any]:
        async with self._lock:
            return await self._run_job(job_id)

    async def probe_source(self, source_id: str) -> dict[str, Any]:
        provider = self._registry.provider(source_id)
        row = self._meta.get_source(source_id) or {}
        notes: list[str] = []
        caps: dict[str, Any] = {}
        ok = True
        detail: dict[str, Any] = {}
        for inst_type in row.get("inst_types") or ["spot"]:
            try:
                cap = await provider.capabilities(inst_type)
                caps[inst_type] = cap.as_api()
                notes.extend(cap.notes or [])
            except Unsupported as exc:
                caps[inst_type] = {"supported": False, "reason": str(exc)}
            except Exception as exc:  # noqa: BLE001 - reported to the UI
                ok = False
                caps[inst_type] = {"supported": False, "error": str(exc)}
                notes.append(f"{inst_type}: {exc}")
        if hasattr(provider, "self_check"):
            detail = await provider.self_check()  # type: ignore[attr-defined]
            ok = ok and bool(detail.get("ok"))
            notes.extend(detail.get("notes") or [])
        status = "ok" if ok else "unreachable"
        self._meta.set_source_status(source_id, status, notes=notes, capabilities=caps)
        return {"source": source_id, "status": status, "capabilities": caps, "notes": notes, "detail": detail}

    # ── internals ──────────────────────────────────────────────────────

    async def _run_job(self, job_id: str) -> dict[str, Any]:
        job = self._meta.get_job(job_id)
        if job is None:
            raise BadRequest(f"任务不存在: {job_id}")

        interval = job["interval"]
        if not is_interval(interval):
            raise BadRequest(f"interval/unsupported: {interval}")
        symbols = [norm_symbol(s) for s in (job.get("symbols") or [])]
        if not symbols:
            raise BadRequest("任务未指定标的")

        inst_type = job.get("inst_type") or "spot"
        price_type = job.get("price_type") or DEFAULT_PRICE_TYPE
        source_id = job["source"]
        provider = self._registry.provider(source_id)
        venue = job.get("venue") or provider.cfg.venue
        if not venue:
            raise BadRequest(f"数据源 {source_id} 未绑定交易所，请在任务里指定 venue")

        batch_id = f"b-{now_ms():x}"
        kind = job.get("mode") or "incremental"
        self._meta.create_batch(batch_id, source=source_id, kind=kind, job_id=job_id)
        self._meta.update_job(job_id, {"status": "running", "last_batch": batch_id, "last_error": None})

        step = interval_ms(interval)
        fetched = written = rejected = 0
        errors: list[str] = []

        for symbol in symbols:
            inst = instrument_id(venue, symbol, inst_type)
            key = dataset_key(venue, symbol, inst_type, "ohlcv", interval, price_type)
            try:
                since, until = self._window(job, inst, interval)
            except BadRequest as exc:
                errors.append(str(exc))
                self._meta.add_batch_item(batch_id, key, from_ts=None, to_ts=None, rows=0, error=str(exc))
                continue

            rows_written = 0
            try:
                cursor = since
                while cursor <= until:
                    page = await self._fetch_page(
                        provider=provider,
                        venue=venue,
                        symbol=symbol,
                        inst_type=inst_type,
                        interval=interval,
                        since=cursor,
                        until=until,
                        page=await self._page_size(provider, inst_type),
                    )
                    if not page:
                        break
                    fetched += len(page)
                    rejected += duplicates(page)
                    bars = [b for b in page if b.closed]
                    if bars:
                        rows_written += self._market.upsert_bars(
                            bars, namespace=DEFAULT_NAMESPACE, batch_id=batch_id
                        )
                    next_cursor = page[-1].ts + step
                    if next_cursor <= cursor:
                        # A venue that keeps returning the same window would
                        # loop forever; stop and record it instead of hanging.
                        errors.append(f"{symbol}: 分页未推进（ts={page[-1].ts}）")
                        break
                    cursor = next_cursor
            except Unsupported as exc:
                errors.append(f"{symbol}: {exc}")
            except ProviderError as exc:
                errors.append(f"{symbol}: {exc}")
            except Exception as exc:  # noqa: BLE001 - one symbol must not kill the batch
                errors.append(f"{symbol}: {type(exc).__name__}: {exc}")

            self._market.upsert_instruments(
                [
                    {
                        "instrument_id": inst,
                        "venue": venue,
                        "symbol": symbol,
                        "venue_symbol": symbol.replace("/", ""),
                        "base": symbol.split("/")[0],
                        "quote": symbol.split("/")[1],
                        "inst_type": inst_type,
                        "contract_size": 1.0,
                        "updated_at": now_ms(),
                    }
                ]
            )
            stats = self._market.stats(DEFAULT_NAMESPACE, inst, interval, price_type)
            self._meta.upsert_dataset(
                {
                    "dataset_key": key,
                    "market": "crypto",
                    "exchange": venue,
                    "venue": venue,
                    "symbol": symbol,
                    "inst_type": inst_type,
                    "interval": interval,
                    "price_type": price_type,
                    "data_type": "ohlcv",
                    "instrument_id": inst,
                    "source": source_id,
                    "namespace": DEFAULT_NAMESPACE,
                    "as_of": stats["to"],
                    "ingested_at": now_ms(),
                    "row_count": stats["rows"],
                    "mode": "backfill",
                }
            )
            self._meta.add_batch_item(
                batch_id, key, from_ts=since, to_ts=until, rows=rows_written,
                error=errors[-1] if errors and errors[-1].startswith(f"{symbol}:") else None,
            )
            written += rows_written
            self._refresh_coverage(key, inst, interval, price_type, since, until)
            self._run_quality(key, inst, interval, price_type)

        status = "ok" if not errors else ("partial" if written else "failed")
        self._meta.finish_batch(
            batch_id,
            status=status,
            fetched=fetched,
            written=written,
            rejected=rejected,
            detail="\n".join(errors[:20]) or None,
        )
        self._meta.update_job(
            job_id,
            {
                "status": "idle" if status == "ok" else "error",
                "last_run_at": now_ms(),
                "rows_written": int(job.get("rows_written") or 0) + written,
                "last_error": "\n".join(errors[:5]) or None,
                "cursor": self._market.last_closed_ts(DEFAULT_NAMESPACE, instrument_id(venue, symbols[0], inst_type), interval, price_type),
            },
        )
        return {
            "batchId": batch_id,
            "status": status,
            "rowsFetched": fetched,
            "rowsWritten": written,
            "rowsRejected": rejected,
            "errors": errors,
        }

    def _window(self, job: dict[str, Any], inst: str, interval: str) -> tuple[int, int]:
        step = interval_ms(interval)
        mode = job.get("mode") or "incremental"
        price_type = job.get("price_type") or DEFAULT_PRICE_TYPE
        until = int(job.get("to_ts") or now_ms())

        if mode == "backfill":
            since = job.get("from_ts")
            if since is None:
                since = until - 7 * 86_400_000
            return int(since), until

        if mode == "incremental":
            last = self._market.last_closed_ts(
                DEFAULT_NAMESPACE, inst, interval, price_type
            )
            # Resume from the bar *after* the last closed one. Re-fetching the
            # last bar would be harmless (primary key), skipping it would leave
            # a permanently stale row if it was written before it closed.
            since = (last + step) if last else int(job.get("from_ts") or until - 7 * 86_400_000)
            return since, until

        if mode == "manual":
            since = job.get("from_ts")
            if since is None:
                raise BadRequest("manual 模式需要 from_ts")
            return int(since), until

        raise BadRequest(f"mode/unsupported: {mode}")

    async def _page_size(self, provider, inst_type: str) -> int:
        try:
            cap = await provider.capabilities(inst_type)
            return max(1, min(cap.max_page, 1500))
        except Exception:  # noqa: BLE001 - fall back to a conservative page
            return 500

    async def _fetch_page(self, *, provider, venue, symbol, inst_type, interval, since, until, page) -> list[Bar]:
        req = OhlcvRequest(
            venue=venue,
            symbol=symbol,
            inst_type=inst_type,
            interval=interval,
            since=since,
            until=until,
            limit=page,
        )
        return await provider.fetch_ohlcv(req)

    def _refresh_coverage(self, key: str, inst: str, interval: str, price_type: str, since: int, until: int) -> None:
        rows = self._market.query(
            "SELECT strftime(epoch_ms(ts), '%Y-%m-%d') AS day, "
            "CAST(strftime(epoch_ms(ts), '%H') AS INTEGER) AS hour, "
            "count(*) AS present FROM ohlcv "
            'WHERE namespace = ? AND instrument_id = ? AND "interval" = ? AND price_type = ? '
            "AND ts >= ? AND ts <= ? GROUP BY 1, 2 ORDER BY 1, 2",
            [DEFAULT_NAMESPACE, inst, interval, price_type, since, until],
        )
        step = interval_ms(interval)
        expected = max(1, 3_600_000 // step)
        self._meta.replace_coverage(
            key, [(r["day"], int(r["hour"]), int(r["present"]), expected) for r in rows]
        )

    def _run_quality(self, key: str, inst: str, interval: str, price_type: str) -> None:
        result = check_bars(self._market, instrument_id=inst, interval=interval, price_type=price_type)
        run_id = f"q-{now_ms():x}"
        self._meta.save_quality_run(run_id, key, result["passed"], result["findings"])
