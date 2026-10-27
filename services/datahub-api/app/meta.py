"""SQLite metadata: data sources, dataset catalog, jobs, coverage, batches.

The source registry lives here (not in code) because adding a venue or pointing
the same venue at our own aggregation API is a configuration change, not a
release. `provider` names the *implementation*; `venue` names the *exchange the
data belongs to*. One venue may be reachable through several sources — direct
CCXT, our official API, a mirror — and rows from all of them share one
`dataset_key` because the key is built from the venue, not the channel.
"""

from __future__ import annotations

import json
import sqlite3
import threading
import time
from pathlib import Path
from typing import Any, Iterable

SCHEMA_VERSION = 2

BASE_DDL = """
CREATE TABLE IF NOT EXISTS sources (
  id               TEXT PRIMARY KEY,
  label            TEXT NOT NULL,
  provider         TEXT NOT NULL,
  venue            TEXT,
  kind             TEXT NOT NULL DEFAULT 'exchange',
  base_urls        TEXT NOT NULL DEFAULT '[]',
  enabled          INTEGER NOT NULL DEFAULT 1,
  priority         INTEGER NOT NULL DEFAULT 100,
  inst_types       TEXT NOT NULL DEFAULT '["spot"]',
  intervals        TEXT,
  auth_kind        TEXT NOT NULL DEFAULT 'none',
  auth_secret_ref  TEXT,
  config           TEXT NOT NULL DEFAULT '{}',
  capabilities     TEXT,
  capabilities_at  INTEGER,
  status           TEXT NOT NULL DEFAULT 'unknown',
  status_at        INTEGER,
  notes            TEXT NOT NULL DEFAULT '[]',
  created_at       INTEGER NOT NULL,
  updated_at       INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS datasets (
  dataset_key  TEXT PRIMARY KEY,
  market       TEXT NOT NULL,
  exchange     TEXT NOT NULL,
  symbol       TEXT NOT NULL,
  interval     TEXT NOT NULL,
  source       TEXT NOT NULL,
  namespace    TEXT NOT NULL DEFAULT 'bronze',
  as_of        INTEGER,
  ingested_at  INTEGER,
  closed_only  INTEGER NOT NULL DEFAULT 1,
  row_count    INTEGER NOT NULL DEFAULT 0,
  created_at   INTEGER NOT NULL,
  updated_at   INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_datasets_market ON datasets(market);
CREATE INDEX IF NOT EXISTS idx_datasets_ns ON datasets(namespace);

CREATE TABLE IF NOT EXISTS sync_jobs (
  id           TEXT PRIMARY KEY,
  dataset_key  TEXT NOT NULL,
  source       TEXT NOT NULL,
  symbols      TEXT NOT NULL,
  interval     TEXT NOT NULL,
  cron         TEXT,
  mode         TEXT NOT NULL DEFAULT 'incremental',
  cursor       INTEGER,
  status       TEXT NOT NULL DEFAULT 'idle',
  last_run_at  INTEGER,
  last_error   TEXT,
  rows_written INTEGER NOT NULL DEFAULT 0,
  enabled      INTEGER NOT NULL DEFAULT 1,
  created_at   INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_jobs_enabled ON sync_jobs(enabled);
CREATE INDEX IF NOT EXISTS idx_jobs_dataset ON sync_jobs(dataset_key);

CREATE TABLE IF NOT EXISTS coverage (
  dataset_key TEXT NOT NULL,
  day         TEXT NOT NULL,
  hour        INTEGER NOT NULL,
  present     INTEGER NOT NULL,
  expected    INTEGER NOT NULL,
  PRIMARY KEY (dataset_key, day, hour)
);
CREATE INDEX IF NOT EXISTS idx_cov_day ON coverage(dataset_key, day);

CREATE TABLE IF NOT EXISTS quality_runs (
  id          TEXT PRIMARY KEY,
  dataset_key TEXT NOT NULL,
  ran_at      INTEGER NOT NULL,
  passed      INTEGER NOT NULL,
  findings    TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_qr_dataset ON quality_runs(dataset_key, ran_at DESC);

CREATE TABLE IF NOT EXISTS imports (
  id          TEXT PRIMARY KEY,
  dataset_key TEXT NOT NULL,
  namespace   TEXT NOT NULL DEFAULT 'imported',
  file        TEXT NOT NULL,
  rows_total  INTEGER NOT NULL DEFAULT 0,
  rows_ok     INTEGER NOT NULL DEFAULT 0,
  rows_bad    INTEGER NOT NULL DEFAULT 0,
  gate_passed INTEGER NOT NULL DEFAULT 0,
  gate_report TEXT,
  created_at  INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_imp_dataset ON imports(dataset_key, created_at DESC);

CREATE TABLE IF NOT EXISTS query_history (
  id          TEXT PRIMARY KEY,
  sql         TEXT NOT NULL,
  ran_at      INTEGER NOT NULL,
  status      TEXT NOT NULL,
  row_count   INTEGER NOT NULL DEFAULT 0,
  duration_ms INTEGER NOT NULL DEFAULT 0,
  error       TEXT
);
CREATE INDEX IF NOT EXISTS idx_qh_ran ON query_history(ran_at DESC);

CREATE TABLE IF NOT EXISTS import_batches (
  id            TEXT PRIMARY KEY,
  source        TEXT NOT NULL,
  channel       TEXT NOT NULL DEFAULT 'rest',
  kind          TEXT NOT NULL DEFAULT 'backfill',
  status        TEXT NOT NULL DEFAULT 'running',
  requested_at  INTEGER NOT NULL,
  finished_at   INTEGER,
  rows_fetched  INTEGER NOT NULL DEFAULT 0,
  rows_written  INTEGER NOT NULL DEFAULT 0,
  rows_rejected INTEGER NOT NULL DEFAULT 0,
  reject_detail TEXT,
  notes         TEXT,
  job_id        TEXT
);
CREATE INDEX IF NOT EXISTS idx_batch_req ON import_batches(requested_at DESC);

CREATE TABLE IF NOT EXISTS batch_items (
  batch_id     TEXT NOT NULL,
  dataset_key  TEXT NOT NULL,
  from_ts      INTEGER,
  to_ts        INTEGER,
  rows_written INTEGER NOT NULL DEFAULT 0,
  error        TEXT,
  PRIMARY KEY (batch_id, dataset_key)
);

CREATE TABLE IF NOT EXISTS quality_findings (
  run_id      TEXT NOT NULL,
  dataset_key TEXT NOT NULL,
  code        TEXT NOT NULL,
  severity    TEXT NOT NULL,
  scope_from  INTEGER,
  scope_to    INTEGER,
  item_count  INTEGER NOT NULL DEFAULT 0,
  detail      TEXT,
  detected_at INTEGER NOT NULL,
  PRIMARY KEY (run_id, dataset_key, code)
);
CREATE INDEX IF NOT EXISTS idx_qf_code ON quality_findings(dataset_key, code, detected_at DESC);
"""

# (table, column, ddl) — applied only when the column is missing, so both a
# brand-new file and the existing Rust-era database converge on one schema.
MIGRATIONS: list[tuple[str, str, str]] = [
    ("datasets", "data_type", "TEXT NOT NULL DEFAULT 'ohlcv'"),
    ("datasets", "instrument_id", "TEXT"),
    ("datasets", "venue", "TEXT"),
    ("datasets", "inst_type", "TEXT NOT NULL DEFAULT 'spot'"),
    ("datasets", "price_type", "TEXT NOT NULL DEFAULT 'trade'"),
    ("datasets", "mode", "TEXT NOT NULL DEFAULT 'backfill'"),
    ("datasets", "health", "TEXT"),
    ("sync_jobs", "venue", "TEXT"),
    ("sync_jobs", "inst_type", "TEXT NOT NULL DEFAULT 'spot'"),
    ("sync_jobs", "from_ts", "INTEGER"),
    ("sync_jobs", "to_ts", "INTEGER"),
    ("sync_jobs", "last_batch", "TEXT"),
    ("sync_jobs", "price_type", "TEXT NOT NULL DEFAULT 'trade'"),
]

SEED_SOURCES: list[dict[str, Any]] = [
    {
        "id": "binance",
        "label": "币安 Binance",
        "provider": "ccxt",
        "venue": "binance",
        "kind": "exchange",
        # Primary domain first, official market-data mirror second. ccxt is
        # told to use whichever answers; a 4xx never triggers a failover.
        "base_urls": ["https://api.binance.com", "https://data-api.binance.vision"],
        # ccxt keeps every endpoint under `urls['api']`; overriding `public`
        # only moves the market-data host, which is exactly what the mirror
        # serves. Listed in preference order, first reachable one wins.
        "config": {
            "url_overrides": [
                {"public": "https://api.binance.com/api/v3"},
                {"public": "https://data-api.binance.vision/api/v3"},
            ]
        },
        "inst_types": ["spot", "swap"],
        "priority": 10,
    },
    {
        "id": "okx",
        "label": "欧易 OKX",
        "provider": "ccxt",
        "venue": "okx",
        "kind": "exchange",
        "base_urls": ["https://www.okx.com"],
        "inst_types": ["spot", "swap"],
        "priority": 20,
    },
    {
        "id": "bybit",
        "label": "Bybit",
        "provider": "ccxt",
        "venue": "bybit",
        "kind": "exchange",
        "base_urls": ["https://api.bybit.com"],
        "inst_types": ["spot", "swap"],
        "priority": 30,
    },
    {
        "id": "official",
        "label": "官方行情 API",
        "provider": "official",
        "venue": None,
        "kind": "aggregate",
        # Left disabled until a base URL is filled in: guessing an endpoint
        # would produce "unreachable" errors that look like a venue outage.
        "base_urls": [],
        "inst_types": ["spot", "swap"],
        "priority": 5,
        "enabled": 0,
    },
    {
        "id": "replay",
        "label": "离线自检源",
        "provider": "replay",
        "venue": "replay",
        "kind": "local",
        "base_urls": [],
        "inst_types": ["spot"],
        "priority": 999,
    },
]


def now_ms() -> int:
    return int(time.time() * 1000)


def _json(value: Any, default: Any) -> Any:
    if value is None:
        return default
    if isinstance(value, (list, dict)):
        return value
    try:
        return json.loads(value)
    except (ValueError, TypeError):
        return default


class MetaStore:
    def __init__(self, path: Path | str) -> None:
        self.path = str(path)
        self._lock = threading.RLock()
        self._conn = sqlite3.connect(self.path, check_same_thread=False)
        self._conn.row_factory = sqlite3.Row
        self._conn.execute("PRAGMA journal_mode=WAL")
        self._conn.execute("PRAGMA foreign_keys=ON")
        self._init_schema()

    # ── plumbing ───────────────────────────────────────────────────────

    def _init_schema(self) -> None:
        with self._lock:
            self._conn.executescript(BASE_DDL)
            for table, column, ddl in MIGRATIONS:
                existing = {r["name"] for r in self._conn.execute(f"PRAGMA table_info({table})")}
                if column not in existing:
                    self._conn.execute(f"ALTER TABLE {table} ADD COLUMN {column} {ddl}")
            self._conn.execute(f"PRAGMA user_version={SCHEMA_VERSION}")
            self._conn.commit()
        self.seed_sources()

    def _rows(self, sql: str, params: Iterable[Any] = ()) -> list[dict[str, Any]]:
        with self._lock:
            cur = self._conn.execute(sql, list(params))
            return [dict(r) for r in cur.fetchall()]

    def _one(self, sql: str, params: Iterable[Any] = ()) -> dict[str, Any] | None:
        rows = self._rows(sql, params)
        return rows[0] if rows else None

    def _write(self, sql: str, params: Iterable[Any] = ()) -> None:
        with self._lock:
            self._conn.execute(sql, list(params))
            self._conn.commit()

    def close(self) -> None:
        with self._lock:
            self._conn.close()

    # ── sources ────────────────────────────────────────────────────────

    def seed_sources(self) -> None:
        """Insert defaults once. `INSERT OR IGNORE` means a user edit — or a
        row they deleted on purpose — always wins over the seed."""
        ts = now_ms()
        with self._lock:
            for s in SEED_SOURCES:
                self._conn.execute(
                    """
                    INSERT OR IGNORE INTO sources
                      (id, label, provider, venue, kind, base_urls, enabled, priority,
                       inst_types, intervals, auth_kind, auth_secret_ref, config,
                       status, notes, created_at, updated_at)
                    VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, NULL, 'none', NULL, ?, 'unknown', '[]', ?, ?)
                    """,
                    (
                        s["id"],
                        s["label"],
                        s["provider"],
                        s["venue"],
                        s.get("kind", "exchange"),
                        json.dumps(s.get("base_urls", [])),
                        int(s.get("enabled", 1)),
                        int(s.get("priority", 100)),
                        json.dumps(s.get("inst_types", ["spot"])),
                        json.dumps(s.get("config") or {}),
                        ts,
                        ts,
                    ),
                )
                # Fill in a default config that did not exist when the row was
                # first seeded, without ever clobbering an operator's edit.
                if s.get("config"):
                    self._conn.execute(
                        "UPDATE sources SET config = ?, updated_at = ? WHERE id = ? AND (config IS NULL OR config = '{}')",
                        (json.dumps(s["config"]), ts, s["id"]),
                    )
            self._conn.commit()

    def _source_row(self, row: sqlite3.Row | dict[str, Any]) -> dict[str, Any]:
        out = dict(row)
        out["base_urls"] = _json(out.get("base_urls"), [])
        out["inst_types"] = _json(out.get("inst_types"), ["spot"])
        out["intervals"] = _json(out.get("intervals"), None)
        out["config"] = _json(out.get("config"), {})
        out["capabilities"] = _json(out.get("capabilities"), None)
        out["notes"] = _json(out.get("notes"), [])
        out["enabled"] = bool(out.get("enabled"))
        return out

    def list_sources(self, include_disabled: bool = True) -> list[dict[str, Any]]:
        sql = "SELECT * FROM sources"
        if not include_disabled:
            sql += " WHERE enabled = 1"
        sql += " ORDER BY priority, id"
        return [self._source_row(r) for r in self._rows(sql)]

    def get_source(self, source_id: str) -> dict[str, Any] | None:
        row = self._one("SELECT * FROM sources WHERE id = ?", [source_id])
        return self._source_row(row) if row else None

    def upsert_source(self, payload: dict[str, Any]) -> dict[str, Any]:
        ts = now_ms()
        sid = payload["id"]
        existing = self.get_source(sid)
        fields = {
            "label": payload.get("label") or (existing or {}).get("label") or sid,
            "provider": payload.get("provider") or (existing or {}).get("provider"),
            "venue": payload.get("venue", (existing or {}).get("venue")),
            "kind": payload.get("kind") or (existing or {}).get("kind") or "exchange",
            "base_urls": payload.get("base_urls", (existing or {}).get("base_urls") or []),
            "enabled": int(bool(payload.get("enabled", (existing or {}).get("enabled", True)))),
            "priority": int(payload.get("priority", (existing or {}).get("priority", 100))),
            "inst_types": payload.get("inst_types", (existing or {}).get("inst_types") or ["spot"]),
            "intervals": payload.get("intervals", (existing or {}).get("intervals")),
            "auth_kind": payload.get("auth_kind") or (existing or {}).get("auth_kind") or "none",
            "auth_secret_ref": payload.get("auth_secret_ref", (existing or {}).get("auth_secret_ref")),
            "config": payload.get("config", (existing or {}).get("config") or {}),
        }
        if not fields["provider"]:
            raise ValueError(f"source/{sid}: provider is required")
        with self._lock:
            self._conn.execute(
                """
                INSERT INTO sources (id, label, provider, venue, kind, base_urls, enabled, priority,
                  inst_types, intervals, auth_kind, auth_secret_ref, config, status, notes,
                  created_at, updated_at)
                VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 'unknown', '[]', ?, ?)
                ON CONFLICT (id) DO UPDATE SET
                  label = excluded.label,
                  provider = excluded.provider,
                  venue = excluded.venue,
                  kind = excluded.kind,
                  base_urls = excluded.base_urls,
                  enabled = excluded.enabled,
                  priority = excluded.priority,
                  inst_types = excluded.inst_types,
                  intervals = excluded.intervals,
                  auth_kind = excluded.auth_kind,
                  auth_secret_ref = excluded.auth_secret_ref,
                  config = excluded.config,
                  updated_at = excluded.updated_at
                """,
                (
                    sid,
                    fields["label"],
                    fields["provider"],
                    fields["venue"],
                    fields["kind"],
                    json.dumps(fields["base_urls"]),
                    fields["enabled"],
                    fields["priority"],
                    json.dumps(fields["inst_types"]),
                    json.dumps(fields["intervals"]) if fields["intervals"] else None,
                    fields["auth_kind"],
                    fields["auth_secret_ref"],
                    json.dumps(fields["config"]),
                    ts,
                    ts,
                ),
            )
            self._conn.commit()
        return self.get_source(sid) or {}

    def delete_source(self, source_id: str) -> bool:
        with self._lock:
            cur = self._conn.execute("DELETE FROM sources WHERE id = ?", [source_id])
            self._conn.commit()
            return cur.rowcount > 0

    def set_source_status(
        self,
        source_id: str,
        status: str,
        notes: list[str] | None = None,
        capabilities: dict[str, Any] | None = None,
    ) -> None:
        ts = now_ms()
        with self._lock:
            if capabilities is not None:
                self._conn.execute(
                    "UPDATE sources SET status=?, status_at=?, notes=?, capabilities=?, capabilities_at=?, updated_at=? WHERE id=?",
                    [status, ts, json.dumps(notes or []), json.dumps(capabilities), ts, ts, source_id],
                )
            elif notes is not None:
                self._conn.execute(
                    "UPDATE sources SET status=?, status_at=?, notes=?, updated_at=? WHERE id=?",
                    [status, ts, json.dumps(notes), ts, source_id],
                )
            else:
                self._conn.execute(
                    "UPDATE sources SET status=?, status_at=?, updated_at=? WHERE id=?",
                    [status, ts, ts, source_id],
                )
            self._conn.commit()

    # ── datasets ───────────────────────────────────────────────────────

    _DATASET_COLS = (
        "dataset_key, market, exchange, symbol, interval, source, namespace, as_of, ingested_at, "
        "closed_only, row_count, created_at, updated_at, data_type, instrument_id, venue, inst_type, "
        "price_type, mode, health"
    )

    def upsert_dataset(self, rec: dict[str, Any]) -> None:
        ts = now_ms()
        payload = {
            "dataset_key": rec["dataset_key"],
            "market": rec.get("market", "crypto"),
            "exchange": rec.get("exchange") or rec.get("venue") or "",
            "symbol": rec.get("symbol", ""),
            "interval": rec.get("interval", ""),
            "source": rec.get("source", ""),
            "namespace": rec.get("namespace", "bronze"),
            "as_of": rec.get("as_of"),
            "ingested_at": rec.get("ingested_at"),
            "closed_only": int(bool(rec.get("closed_only", True))),
            "row_count": int(rec.get("row_count") or 0),
            "data_type": rec.get("data_type", "ohlcv"),
            "instrument_id": rec.get("instrument_id"),
            "venue": rec.get("venue"),
            "inst_type": rec.get("inst_type", "spot"),
            "price_type": rec.get("price_type", "trade"),
            "mode": rec.get("mode", "backfill"),
            "health": rec.get("health"),
        }
        with self._lock:
            self._conn.execute(
                """
                INSERT INTO datasets (dataset_key, market, exchange, symbol, interval, source,
                  namespace, as_of, ingested_at, closed_only, row_count, created_at, updated_at,
                  data_type, instrument_id, venue, inst_type, price_type, mode, health)
                VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                ON CONFLICT (dataset_key) DO UPDATE SET
                  market = excluded.market,
                  exchange = excluded.exchange,
                  symbol = excluded.symbol,
                  interval = excluded.interval,
                  source = excluded.source,
                  namespace = excluded.namespace,
                  as_of = excluded.as_of,
                  ingested_at = excluded.ingested_at,
                  closed_only = excluded.closed_only,
                  row_count = excluded.row_count,
                  updated_at = excluded.updated_at,
                  data_type = excluded.data_type,
                  instrument_id = excluded.instrument_id,
                  venue = excluded.venue,
                  inst_type = excluded.inst_type,
                  price_type = excluded.price_type,
                  mode = excluded.mode,
                  health = COALESCE(excluded.health, datasets.health)
                """,
                [payload[c] for c in payload]
                if False
                else [
                    payload["dataset_key"], payload["market"], payload["exchange"], payload["symbol"],
                    payload["interval"], payload["source"], payload["namespace"], payload["as_of"],
                    payload["ingested_at"], payload["closed_only"], payload["row_count"], ts, ts,
                    payload["data_type"], payload["instrument_id"], payload["venue"],
                    payload["inst_type"], payload["price_type"], payload["mode"], payload["health"],
                ],
            )
            self._conn.commit()

    def list_datasets(self, market: str | None = None, namespace: str | None = None) -> list[dict[str, Any]]:
        sql = f"SELECT {self._DATASET_COLS} FROM datasets WHERE 1=1"
        params: list[Any] = []
        if market:
            sql += " AND market = ?"
            params.append(market)
        if namespace:
            sql += " AND namespace = ?"
            params.append(namespace)
        sql += " ORDER BY dataset_key"
        return self._rows(sql, params)

    def get_dataset(self, key: str) -> dict[str, Any] | None:
        return self._one(f"SELECT {self._DATASET_COLS} FROM datasets WHERE dataset_key = ?", [key])

    # ── jobs ───────────────────────────────────────────────────────────

    def _job_row(self, row: dict[str, Any]) -> dict[str, Any]:
        out = dict(row)
        out["symbols"] = _json(out.get("symbols"), [])
        out["enabled"] = bool(out.get("enabled"))
        return out

    def list_jobs(self, enabled_only: bool = False, dataset_key: str | None = None) -> list[dict[str, Any]]:
        sql = "SELECT * FROM sync_jobs WHERE 1=1"
        params: list[Any] = []
        if enabled_only:
            sql += " AND enabled = 1"
        if dataset_key:
            sql += " AND dataset_key = ?"
            params.append(dataset_key)
        sql += " ORDER BY created_at DESC"
        return [self._job_row(r) for r in self._rows(sql, params)]

    def get_job(self, job_id: str) -> dict[str, Any] | None:
        row = self._one("SELECT * FROM sync_jobs WHERE id = ?", [job_id])
        return self._job_row(row) if row else None

    def create_job(self, job: dict[str, Any]) -> str:
        ts = now_ms()
        job_id = job.get("id") or f"job-{ts:x}"
        with self._lock:
            self._conn.execute(
                """
                INSERT INTO sync_jobs (id, dataset_key, source, symbols, interval, cron, mode,
                  cursor, status, last_run_at, last_error, rows_written, enabled, created_at,
                  venue, inst_type, from_ts, to_ts, last_batch, price_type)
                VALUES (?, ?, ?, ?, ?, ?, ?, NULL, 'idle', NULL, NULL, 0, ?, ?, ?, ?, ?, ?, NULL, ?)
                ON CONFLICT (id) DO UPDATE SET
                  dataset_key = excluded.dataset_key,
                  source = excluded.source,
                  symbols = excluded.symbols,
                  interval = excluded.interval,
                  mode = excluded.mode,
                  enabled = excluded.enabled,
                  venue = excluded.venue,
                  inst_type = excluded.inst_type,
                  from_ts = excluded.from_ts,
                  to_ts = excluded.to_ts,
                  price_type = excluded.price_type
                """,
                (
                    job_id,
                    job["dataset_key"],
                    job["source"],
                    json.dumps(job.get("symbols") or []),
                    job["interval"],
                    job.get("cron"),
                    job.get("mode", "incremental"),
                    int(bool(job.get("enabled", True))),
                    ts,
                    job.get("venue"),
                    job.get("inst_type", "spot"),
                    job.get("from_ts"),
                    job.get("to_ts"),
                    job.get("price_type", "trade"),
                ),
            )
            self._conn.commit()
        return job_id

    def update_job(self, job_id: str, patch: dict[str, Any]) -> None:
        allowed = {
            "dataset_key", "source", "interval", "mode", "enabled", "venue", "inst_type",
            "from_ts", "to_ts", "price_type", "status", "last_error", "cursor", "rows_written",
            "last_run_at", "last_batch", "symbols",
        }
        sets: list[str] = []
        params: list[Any] = []
        for key, value in patch.items():
            if key not in allowed:
                continue
            if key == "symbols":
                value = json.dumps(value)
            if key == "enabled":
                value = int(bool(value))
            sets.append(f"{key} = ?")
            params.append(value)
        if not sets:
            return
        params.append(job_id)
        self._write(f"UPDATE sync_jobs SET {', '.join(sets)} WHERE id = ?", params)

    def delete_job(self, job_id: str) -> bool:
        with self._lock:
            cur = self._conn.execute("DELETE FROM sync_jobs WHERE id = ?", [job_id])
            self._conn.commit()
            return cur.rowcount > 0

    # ── batches ────────────────────────────────────────────────────────

    def create_batch(
        self,
        batch_id: str,
        *,
        source: str,
        kind: str,
        job_id: str | None = None,
        channel: str = "rest",
        notes: str | None = None,
    ) -> None:
        self._write(
            "INSERT INTO import_batches (id, source, channel, kind, status, requested_at, job_id, notes) "
            "VALUES (?, ?, ?, ?, 'running', ?, ?, ?)",
            [batch_id, source, channel, kind, now_ms(), job_id, notes],
        )

    def finish_batch(
        self,
        batch_id: str,
        *,
        status: str,
        fetched: int = 0,
        written: int = 0,
        rejected: int = 0,
        detail: str | None = None,
    ) -> None:
        self._write(
            "UPDATE import_batches SET status=?, finished_at=?, rows_fetched=?, rows_written=?, "
            "rows_rejected=?, reject_detail=? WHERE id=?",
            [status, now_ms(), fetched, written, rejected, detail, batch_id],
        )

    def add_batch_item(
        self,
        batch_id: str,
        dataset_key: str,
        *,
        from_ts: int | None,
        to_ts: int | None,
        rows: int,
        error: str | None = None,
    ) -> None:
        self._write(
            "INSERT INTO batch_items (batch_id, dataset_key, from_ts, to_ts, rows_written, error) "
            "VALUES (?, ?, ?, ?, ?, ?) "
            "ON CONFLICT (batch_id, dataset_key) DO UPDATE SET from_ts=excluded.from_ts, "
            "to_ts=excluded.to_ts, rows_written=batch_items.rows_written + excluded.rows_written, "
            "error=excluded.error",
            [batch_id, dataset_key, from_ts, to_ts, rows, error],
        )

    def list_batches(self, limit: int = 50, status: str | None = None) -> list[dict[str, Any]]:
        sql = "SELECT * FROM import_batches"
        params: list[Any] = []
        if status:
            sql += " WHERE status = ?"
            params.append(status)
        sql += " ORDER BY requested_at DESC LIMIT ?"
        params.append(max(1, min(limit, 500)))
        return self._rows(sql, params)

    def batch_items(self, batch_id: str) -> list[dict[str, Any]]:
        return self._rows("SELECT * FROM batch_items WHERE batch_id = ?", [batch_id])

    # ── coverage / quality ─────────────────────────────────────────────

    def replace_coverage(self, dataset_key: str, cells: list[tuple[str, int, int, int]]) -> None:
        with self._lock:
            self._conn.execute("DELETE FROM coverage WHERE dataset_key = ?", [dataset_key])
            if cells:
                self._conn.executemany(
                    "INSERT INTO coverage (dataset_key, day, hour, present, expected) VALUES (?, ?, ?, ?, ?)",
                    [(dataset_key, d, h, p, e) for d, h, p, e in cells],
                )
            self._conn.commit()

    def coverage(self, dataset_key: str, month: str | None = None) -> list[dict[str, Any]]:
        sql = "SELECT * FROM coverage WHERE dataset_key = ?"
        params: list[Any] = [dataset_key]
        if month:
            sql += " AND day LIKE ?"
            params.append(f"{month}%")
        sql += " ORDER BY day, hour"
        return self._rows(sql, params)

    def save_quality_run(self, run_id: str, dataset_key: str, passed: bool, findings: list[dict[str, Any]]) -> None:
        ts = now_ms()
        with self._lock:
            self._conn.execute(
                "INSERT OR REPLACE INTO quality_runs (id, dataset_key, ran_at, passed, findings) VALUES (?, ?, ?, ?, ?)",
                [run_id, dataset_key, ts, int(passed), json.dumps(findings)],
            )
            self._conn.execute("DELETE FROM quality_findings WHERE run_id = ? AND dataset_key = ?", [run_id, dataset_key])
            self._conn.executemany(
                "INSERT INTO quality_findings (run_id, dataset_key, code, severity, scope_from, scope_to, "
                "item_count, detail, detected_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
                [
                    (
                        run_id,
                        dataset_key,
                        f["code"],
                        f["severity"],
                        f.get("scopeFrom"),
                        f.get("scopeTo"),
                        int(f.get("itemCount") or 0),
                        json.dumps(f.get("detail")) if f.get("detail") is not None else None,
                        ts,
                    )
                    for f in findings
                ],
            )
            health = "ok" if passed else ("error" if any(f["severity"] == "error" for f in findings) else "warn")
            self._conn.execute("UPDATE datasets SET health = ?, updated_at = ? WHERE dataset_key = ?", [health, ts, dataset_key])
            self._conn.commit()

    def latest_quality(self, dataset_key: str | None = None) -> dict[str, Any] | None:
        if dataset_key:
            row = self._one(
                "SELECT * FROM quality_runs WHERE dataset_key = ? ORDER BY ran_at DESC LIMIT 1",
                [dataset_key],
            )
        else:
            row = self._one("SELECT * FROM quality_runs ORDER BY ran_at DESC LIMIT 1")
        if not row:
            return None
        row["findings"] = _json(row.get("findings"), [])
        row["passed"] = bool(row.get("passed"))
        return row

    def good_findings(self, limit: int = 100) -> list[dict[str, Any]]:
        return self._rows(
            "SELECT * FROM quality_findings ORDER BY detected_at DESC LIMIT ?", [max(1, min(limit, 500))]
        )

    # ── query history ──────────────────────────────────────────────────

    def log_query(self, sql: str, status: str, row_count: int, duration_ms: int, error: str | None = None) -> None:
        self._write(
            "INSERT INTO query_history (id, sql, ran_at, status, row_count, duration_ms, error) "
            "VALUES (?, ?, ?, ?, ?, ?, ?)",
            [f"q-{now_ms():x}", sql, now_ms(), status, row_count, duration_ms, error],
        )

    def query_history(self, limit: int = 30) -> list[dict[str, Any]]:
        return self._rows("SELECT * FROM query_history ORDER BY ran_at DESC LIMIT ?", [max(1, min(limit, 200))])

    def log_import(self, rec: dict[str, Any]) -> None:
        self._write(
            "INSERT INTO imports (id, dataset_key, namespace, file, rows_total, rows_ok, rows_bad, "
            "gate_passed, gate_report, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            [
                rec["id"],
                rec["dataset_key"],
                rec.get("namespace", "imported"),
                rec.get("file", ""),
                rec.get("rows_total", 0),
                rec.get("rows_ok", 0),
                rec.get("rows_bad", 0),
                int(bool(rec.get("gate_passed", False))),
                rec.get("gate_report"),
                now_ms(),
            ],
        )

    def list_imports(self, limit: int = 50) -> list[dict[str, Any]]:
        return self._rows("SELECT * FROM imports ORDER BY created_at DESC LIMIT ?", [max(1, min(limit, 200))])
