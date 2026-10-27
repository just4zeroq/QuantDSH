"""End-to-end smoke test over the ASGI app (no network, no HTTP server).

Exercises the path the workbench actually uses: list sources → create job →
run job → read bars → read catalog → read quality → run SQL.
"""

from __future__ import annotations

import os
import shutil
import sys
import tempfile
from pathlib import Path

from fastapi.testclient import TestClient

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from app.api import AppState, create_app  # noqa: E402
from app.db import MarketStore  # noqa: E402
from app.meta import MetaStore  # noqa: E402
from app.providers import Registry  # noqa: E402
from app.sync import SyncService  # noqa: E402

TOKEN = "test-token"


def main() -> int:
    data_dir = Path(tempfile.mkdtemp(prefix="quantdsh-py-smoke-"))
    market = MarketStore(data_dir / "market.duckdb")
    meta = MetaStore(data_dir / "meta.sqlite")
    registry = Registry(meta)
    sync = SyncService(market, meta, registry)
    app = create_app(AppState(market=market, meta=meta, registry=registry, sync=sync, token=TOKEN, data_dir=str(data_dir)))

    headers = {"Authorization": f"Bearer {TOKEN}"}
    failures: list[str] = []

    def check(name: str, ok: bool, detail: object = "") -> None:
        print(f"{'PASS' if ok else 'FAIL'}  {name}{'' if ok else f'  -> {detail}'}")
        if not ok:
            failures.append(name)

    with TestClient(app) as client:
        r = client.get("/health")
        check("health (no token)", r.status_code == 200, r.text)

        r = client.get("/api/sources")
        check("unauthorized without token", r.status_code == 401, r.status_code)

        r = client.get("/api/sources", headers=headers)
        sources = r.json()
        ids = sources.get("sources", [])
        all_ids = [s["id"] for s in sources.get("all", [])]
        check("sources seeded", r.status_code == 200 and {"binance", "okx", "bybit", "replay"} <= set(ids), ids)
        check("disabled source kept in registry", "official" in all_ids, all_ids)

        r = client.get("/api/markets", headers=headers)
        check("markets", r.json() == {"markets": ["crypto"]}, r.text)

        r = client.get("/api/schema", headers=headers)
        names = [t["name"] for t in r.json()]
        check("schema has ohlcv", "ohlcv" in names, names)

        # replay source: deterministic, offline, exercises the real write path
        r = client.post(
            "/api/jobs",
            headers=headers,
            json={
                "id": "job-replay-1m",
                "source": "replay",
                "symbols": ["BTC/USDT"],
                "interval": "1m",
                "inst_type": "spot",
                "mode": "backfill",
                "from_ts": 1_700_000_000_000 // 60_000 * 60_000,
                "to_ts": 1_700_000_000_000 // 60_000 * 60_000 + 3_600_000,
            },
        )
        check("create job", r.status_code == 200, r.text)

        r = client.post("/api/jobs/job-replay-1m/run", headers=headers, json={})
        outcome = r.json() if r.status_code == 200 else {}
        check("run job", r.status_code == 200 and outcome.get("rows_written") == 61, r.text)

        r = client.get("/api/bars", headers=headers, params={"symbol": "BTC/USDT", "interval": "1m", "from": 1_700_000_000_000 // 60_000 * 60_000, "to": 1_700_000_000_000 // 60_000 * 60_000 + 3_600_000, "limit": 100})
        bars = r.json()
        check("bars readable", r.status_code == 200 and len(bars) == 61, f"{r.status_code} {len(bars) if r.status_code == 200 else r.text}")
        if bars:
            first = bars[0]
            check(
                "bar wire shape",
                {"symbol", "interval", "ts", "open", "high", "low", "close", "closed"} <= set(first),
                sorted(first),
            )

        r = client.get("/api/datasets", headers=headers)
        datasets = r.json()
        check("dataset registered", r.status_code == 200 and any(d["symbol"] == "BTC/USDT" for d in datasets), datasets)

        r = client.get("/api/coverage/summary", headers=headers, params={"dataset_key": "crypto:replay:BTC/USDT:spot:ohlcv:1m"})
        check("coverage computed", r.status_code == 200 and r.json()["present"] > 0, r.text)

        r = client.get("/api/quality/latest", headers=headers, params={"dataset_key": "crypto:replay:BTC/USDT:spot:ohlcv:1m"})
        run = r.json().get("run")
        check("quality recorded", run is not None, r.text)

        r = client.post("/api/query", headers=headers, json={"sql": "SELECT count(*) AS n FROM ohlcv"})
        q = r.json()
        check("select allowed", q.get("status") == "ok" and q["rows"][0][0] == 61, q)

        r = client.post("/api/query", headers=headers, json={"sql": "DELETE FROM ohlcv"})
        check("delete blocked", r.json().get("status") == "blocked", r.text)

        r = client.post("/api/query", headers=headers, json={"sql": "SELECT 'delete from x' AS s"})
        check("literal not blocked", r.json().get("status") == "ok", r.text)

        r = client.get("/api/jobs", headers=headers)
        jobs = r.json()
        check("job listed", r.status_code == 200 and jobs and jobs[0]["status"] == "idle", jobs)

        r = client.get("/api/batches", headers=headers)
        batches = r.json().get("batches") or []
        check("batch audited", bool(batches) and batches[0]["status"] == "ok", batches)

    market.close()
    meta.close()
    shutil.rmtree(data_dir, ignore_errors=True)
    print()
    if failures:
        print(f"{len(failures)} check(s) failed: {', '.join(failures)}")
        return 1
    print("all checks passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())


