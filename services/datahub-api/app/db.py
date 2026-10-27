"""DuckDB market store: instruments + ohlcv (bars).

DuckDB allows a single writer per file, and the Python driver is not thread
safe, so every statement goes through one re-entrant lock. The service is
expected to be the only writer anyway — external tools must open the file with
`read_only=True` (the query endpoint enforces the same rule for user SQL).
"""

from __future__ import annotations

import csv
import tempfile
import threading
from pathlib import Path
from typing import Any, Iterable, Sequence

import duckdb

from .domain import Bar, DEFAULT_NAMESPACE, DEFAULT_PRICE_TYPE, instrument_id, norm_symbol

OHLCV_DDL = """
CREATE TABLE IF NOT EXISTS ohlcv (
  instrument_id VARCHAR NOT NULL,
  "interval"    VARCHAR NOT NULL,
  price_type    VARCHAR NOT NULL DEFAULT 'trade',
  ts            BIGINT  NOT NULL,
  open   DOUBLE, high DOUBLE, low DOUBLE, close DOUBLE,
  volume   DOUBLE,
  turnover DOUBLE,
  trades_count     BIGINT,
  taker_buy_volume DOUBLE,
  vwap             DOUBLE,
  closed   BOOLEAN NOT NULL DEFAULT TRUE,
  source   VARCHAR NOT NULL DEFAULT 'unknown',
  namespace VARCHAR NOT NULL DEFAULT 'bronze',
  batch_id VARCHAR,
  ingested_at BIGINT NOT NULL,
  PRIMARY KEY (namespace, instrument_id, "interval", price_type, ts)
);
CREATE INDEX IF NOT EXISTS idx_ohlcv_iv ON ohlcv("interval", ts);
CREATE INDEX IF NOT EXISTS idx_ohlcv_inst ON ohlcv(instrument_id);
"""

INSTRUMENTS_DDL = """
CREATE TABLE IF NOT EXISTS instruments (
  instrument_id   VARCHAR PRIMARY KEY,
  market          VARCHAR NOT NULL DEFAULT 'crypto',
  venue           VARCHAR NOT NULL,
  symbol          VARCHAR NOT NULL,
  venue_symbol    VARCHAR NOT NULL,
  base            VARCHAR,
  quote           VARCHAR,
  settle          VARCHAR,
  inst_type       VARCHAR NOT NULL DEFAULT 'spot',
  contract_size   DOUBLE NOT NULL DEFAULT 1,
  tick_size       DOUBLE,
  lot_size        DOUBLE,
  active          BOOLEAN NOT NULL DEFAULT TRUE,
  raw             JSON,
  updated_at      BIGINT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_inst_venue ON instruments(venue, inst_type);
"""

# Rows per `executemany` for the small tables (instruments). The bar path uses
# COPY instead — see `upsert_bars` for why.
_CHUNK = 5000


def _csv_cell(value: Any) -> Any:
    """Booleans become `true`/`false`; NULL becomes an empty field.

    `NULLSTR ''` on the COPY side turns those empty fields back into NULL, so a
    missing trade count never turns into a zero.
    """
    if value is None:
        return ""
    if isinstance(value, bool):
        return "true" if value else "false"
    return value


class MarketStore:
    def __init__(self, path: Path | str) -> None:
        self.path = str(path)
        self._lock = threading.RLock()
        self._conn = duckdb.connect(self.path)
        self._init_schema()

    # ── plumbing ───────────────────────────────────────────────────────

    @property
    def conn(self) -> duckdb.DuckDBPyConnection:
        return self._conn

    def _init_schema(self) -> None:
        with self._lock:
            self._conn.execute(OHLCV_DDL)
            self._conn.execute(INSTRUMENTS_DDL)

    def query(self, sql: str, params: Sequence[Any] | None = None) -> list[dict[str, Any]]:
        with self._lock:
            cur = self._conn.execute(sql, list(params or []))
            cols = [d[0] for d in cur.description] if cur.description else []
            rows = cur.fetchall()
        return [dict(zip(cols, row)) for row in rows]

    def execute(self, sql: str, params: Sequence[Any] | None = None) -> None:
        with self._lock:
            self._conn.execute(sql, list(params or []))

    def close(self) -> None:
        with self._lock:
            self._conn.close()

    # ── instrument ─────────────────────────────────────────────────────

    def upsert_instruments(self, rows: Iterable[dict[str, Any]]) -> int:
        payload = [
            (
                r["instrument_id"],
                r.get("market", "crypto"),
                r["venue"],
                r["symbol"],
                r.get("venue_symbol", r["symbol"].replace("/", "")),
                r.get("base"),
                r.get("quote"),
                r.get("settle"),
                r.get("inst_type", "spot"),
                float(r.get("contract_size") or 1.0),
                r.get("tick_size"),
                r.get("lot_size"),
                bool(r.get("active", True)),
                r.get("raw"),
                int(r.get("updated_at", 0)),
            )
            for r in rows
        ]
        if not payload:
            return 0
        with self._lock:
            for i in range(0, len(payload), _CHUNK):
                self._conn.executemany(
                    """
                    INSERT INTO instruments (instrument_id, market, venue, symbol, venue_symbol,
                      base, quote, settle, inst_type, contract_size, tick_size, lot_size,
                      active, raw, updated_at)
                    VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                    ON CONFLICT (instrument_id) DO UPDATE SET
                      venue_symbol = excluded.venue_symbol,
                      settle = excluded.settle,
                      contract_size = excluded.contract_size,
                      tick_size = excluded.tick_size,
                      lot_size = excluded.lot_size,
                      active = excluded.active,
                      raw = excluded.raw,
                      updated_at = excluded.updated_at
                    """,
                    payload[i : i + _CHUNK],
                )
        return len(payload)

    def instrument(self, inst: str) -> dict[str, Any] | None:
        rows = self.query("SELECT * FROM instruments WHERE instrument_id = ?", [inst])
        return rows[0] if rows else None

    def instruments(self, venue: str | None = None, inst_type: str | None = None) -> list[dict[str, Any]]:
        sql = "SELECT * FROM instruments WHERE 1=1"
        params: list[Any] = []
        if venue:
            sql += " AND venue = ?"
            params.append(venue)
        if inst_type:
            sql += " AND inst_type = ?"
            params.append(inst_type)
        sql += " ORDER BY venue, inst_type, symbol"
        return self.query(sql, params)

    # ── ohlcv ──────────────────────────────────────────────────────────

    def upsert_bars(
        self,
        bars: Iterable[Bar],
        *,
        namespace: str = DEFAULT_NAMESPACE,
        batch_id: str | None = None,
    ) -> int:
        from .domain import now_ms

        ingested = now_ms()
        payload = [
            (
                b.instrument_id,
                b.interval,
                b.price_type or DEFAULT_PRICE_TYPE,
                b.ts,
                b.open,
                b.high,
                b.low,
                b.close,
                b.volume,
                b.turnover,
                b.trades_count,
                b.taker_buy_volume,
                (b.turnover / b.volume) if (b.turnover and b.volume) else None,
                b.closed,
                b.source,
                b.namespace or namespace,
                batch_id,
                b.ingested_at or ingested,
            )
            for b in bars
        ]
        if not payload:
            return 0
        # Load through a temporary CSV + COPY.
        #
        # Why not `executemany`: this DuckDB build (1.5.6, the engine the Rust
        # daemon also pinned, so the on-disk format matches) aborts the process
        # — no Python exception, just a dead interpreter — once a parameterised
        # insert crosses roughly ten thousand rows. COPY bypasses parameter
        # binding entirely and measured 512k rows in 5.8 s.
        #
        # Idempotency comes from deleting the touched ts ranges first: the
        # primary key alone cannot be trusted while the insert is a bulk load.
        groups: dict[tuple[str, str, str, str], tuple[int, int]] = {}
        for row in payload:
            key = (row[15], row[0], row[1], row[2])
            ts = row[3]
            lo, hi = groups.get(key, (ts, ts))
            groups[key] = (min(lo, ts), max(hi, ts))

        with self._lock:
            handle = tempfile.NamedTemporaryFile(
                "w", suffix=".csv", delete=False, newline="", encoding="utf-8"
            )
            try:
                writer = csv.writer(handle)
                for row in payload:
                    writer.writerow([_csv_cell(value) for value in row])
                handle.close()
                path = handle.name.replace("\\", "/")

                self._conn.execute("BEGIN TRANSACTION")
                try:
                    self._conn.execute(
                        """
                        CREATE OR REPLACE TEMP TABLE _ohlcv_stage (
                          instrument_id VARCHAR, "interval" VARCHAR, price_type VARCHAR, ts BIGINT,
                          open DOUBLE, high DOUBLE, low DOUBLE, close DOUBLE, volume DOUBLE, turnover DOUBLE,
                          trades_count BIGINT, taker_buy_volume DOUBLE, vwap DOUBLE, closed BOOLEAN,
                          source VARCHAR, namespace VARCHAR, batch_id VARCHAR, ingested_at BIGINT
                        )
                        """
                    )
                    self._conn.execute(
                        f"COPY _ohlcv_stage FROM '{path}' (FORMAT CSV, HEADER false, NULLSTR '', QUOTE '\"')"
                    )
                    for (ns, inst, interval, price_type), (lo, hi) in groups.items():
                        self._conn.execute(
                            'DELETE FROM ohlcv WHERE namespace = ? AND instrument_id = ? '
                            'AND "interval" = ? AND price_type = ? AND ts BETWEEN ? AND ?',
                            [ns, inst, interval, price_type, lo, hi],
                        )
                    self._conn.execute("INSERT INTO ohlcv SELECT * FROM _ohlcv_stage")
                    self._conn.execute("COMMIT")
                except Exception:
                    self._conn.execute("ROLLBACK")
                    raise
            finally:
                handle.close()
                Path(handle.name).unlink(missing_ok=True)
        return len(payload)

    def bars(
        self,
        *,
        instrument_id: str | None = None,
        symbol: str | None = None,
        interval: str | None = None,
        namespace: str = DEFAULT_NAMESPACE,
        price_type: str = DEFAULT_PRICE_TYPE,
        start: int | None = None,
        end: int | None = None,
        limit: int = 5000,
        ascending: bool = True,
    ) -> list[dict[str, Any]]:
        sql = (
            'SELECT instrument_id, "interval", price_type, ts, open, high, low, close, '
            "volume, turnover, trades_count, taker_buy_volume, closed, source, namespace, ingested_at "
            "FROM ohlcv WHERE namespace = ? AND price_type = ?"
        )
        params: list[Any] = [namespace, price_type]
        if instrument_id:
            sql += " AND instrument_id = ?"
            params.append(instrument_id)
        elif symbol:
            # Callers that only know a symbol (the chart, the CSV importer) get
            # a cross-venue match. Acceptable at this size; the instrument id is
            # the precise path and the UI uses it whenever it has one.
            sql += " AND instrument_id LIKE ?"
            params.append(f"%:{norm_symbol(symbol)}:%")
        if interval:
            sql += ' AND "interval" = ?'
            params.append(interval)
        if start is not None:
            sql += " AND ts >= ?"
            params.append(start)
        if end is not None:
            sql += " AND ts <= ?"
            params.append(end)
        sql += " ORDER BY ts " + ("ASC" if ascending else "DESC") + " LIMIT ?"
        params.append(max(1, min(limit, 200_000)))
        return self.query(sql, params)

    def timestamps(
        self,
        *,
        instrument_id: str | None = None,
        symbol: str | None = None,
        interval: str | None = None,
        namespace: str = DEFAULT_NAMESPACE,
        start: int | None = None,
        end: int | None = None,
    ) -> list[int]:
        sql = "SELECT ts FROM ohlcv WHERE namespace = ?"
        params: list[Any] = [namespace]
        if instrument_id:
            sql += " AND instrument_id = ?"
            params.append(instrument_id)
        elif symbol:
            sql += " AND instrument_id LIKE ?"
            params.append(f"%:{norm_symbol(symbol)}:%")
        if interval:
            sql += ' AND "interval" = ?'
            params.append(interval)
        if start is not None:
            sql += " AND ts >= ?"
            params.append(start)
        if end is not None:
            sql += " AND ts <= ?"
            params.append(end)
        sql += " ORDER BY ts"
        return [r["ts"] for r in self.query(sql, params)]

    def stats(self, namespace: str, inst: str, interval: str, price_type: str = DEFAULT_PRICE_TYPE) -> dict[str, Any]:
        rows = self.query(
            'SELECT count(*) AS n, min(ts) AS lo, max(ts) AS hi, '
            'count(*) FILTER (WHERE closed) AS closed_n '
            'FROM ohlcv WHERE namespace = ? AND instrument_id = ? AND "interval" = ? AND price_type = ?',
            [namespace, inst, interval, price_type],
        )
        row = rows[0] if rows else {}
        return {
            "rows": int(row.get("n") or 0),
            "from": row.get("lo"),
            "to": row.get("hi"),
            "closed": int(row.get("closed_n") or 0),
        }

    def last_closed_ts(self, namespace: str, inst: str, interval: str, price_type: str = DEFAULT_PRICE_TYPE) -> int | None:
        rows = self.query(
            'SELECT max(ts) AS ts FROM ohlcv WHERE namespace = ? AND instrument_id = ? '
            'AND "interval" = ? AND price_type = ? AND closed',
            [namespace, inst, interval, price_type],
        )
        return rows[0]["ts"] if rows and rows[0]["ts"] is not None else None

    def delete_bars(
        self,
        *,
        instrument_id: str,
        interval: str,
        namespace: str = DEFAULT_NAMESPACE,
        start: int | None = None,
        end: int | None = None,
    ) -> None:
        sql = 'DELETE FROM ohlcv WHERE namespace = ? AND instrument_id = ? AND "interval" = ?'
        params: list[Any] = [namespace, instrument_id, interval]
        if start is not None:
            sql += " AND ts >= ?"
            params.append(start)
        if end is not None:
            sql += " AND ts <= ?"
            params.append(end)
        self.execute(sql, params)

    # ── schema / migration ─────────────────────────────────────────────

    def table_exists(self, name: str) -> bool:
        rows = self.query(
            "SELECT table_name FROM information_schema.tables WHERE table_name = ?", [name]
        )
        return bool(rows)

    def schema(self) -> list[dict[str, Any]]:
        out: list[dict[str, Any]] = []
        for table in ("ohlcv", "instruments", "bars", "trades"):
            if not self.table_exists(table):
                continue
            cols = self.query(f'DESCRIBE "{table}"')
            out.append(
                {
                    "name": table,
                    "columns": [
                        {
                            "name": c["column_name"],
                            "type": c["column_type"],
                            "nullable": c.get("null") in ("YES", True, "true"),
                        }
                        for c in cols
                    ],
                }
            )
        return out

    def migrate_legacy_bars(self, venue: str = "binance") -> int:
        """Copy `bars` into `ohlcv` once.

        The legacy table has no exchange column, so every row is attributed to
        `venue` (the only writer that existed before this service). Runs only
        when `ohlcv` is empty, so a restart can never duplicate rows.
        """
        if not self.table_exists("bars"):
            return 0
        if self.query("SELECT count(*) AS n FROM ohlcv")[0]["n"] > 0:
            return 0
        legacy = self.query(
            'SELECT symbol, "interval", ts, open, high, low, close, volume, turnover, '
            "closed, source, namespace, ingested_at FROM bars ORDER BY ts"
        )
        if not legacy:
            return 0
        bars: list[Bar] = []
        for row in legacy:
            try:
                inst = instrument_id(venue, row["symbol"], "spot")
            except ValueError:
                continue
            bars.append(
                Bar(
                    symbol=norm_symbol(row["symbol"]),
                    interval=row["interval"],
                    ts=row["ts"],
                    open=row["open"],
                    high=row["high"],
                    low=row["low"],
                    close=row["close"],
                    volume=row["volume"],
                    turnover=row["turnover"],
                    closed=bool(row["closed"]),
                    source=row["source"],
                    namespace=row["namespace"],
                    ingested_at=row["ingested_at"],
                    instrument_id=inst,
                )
            )
        return self.upsert_bars(bars)
