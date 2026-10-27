"""Entry point: `python -m app --data-dir DIR [--port N] [--token T]`.

Mirrors the Rust daemon's CLI and port-file behaviour so the Tauri carrier does
not need to know which language the data service is written in: it spawns a
process, waits for `datahub.json`, and injects `{baseUrl, token}` into the
webview.
"""

from __future__ import annotations

import argparse
import json
import os
import sys
from pathlib import Path

import uvicorn

from .api import AppState, create_app
from .config import DEFAULT_PORT, acquire, default_data_dir, release
from .db import MarketStore
from .meta import MetaStore
from .providers import Registry
from .sync import SyncService


def _parse(argv: list[str] | None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(prog="quantdsh-datahub-py", description="QuantDSH market data service")
    parser.add_argument("--data-dir", default=None)
    parser.add_argument("--port", type=int, default=DEFAULT_PORT)
    parser.add_argument("--token", default=None)
    parser.add_argument("--resolve-only", action="store_true")
    return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    args = _parse(argv)
    data_dir = Path(args.data_dir) if args.data_dir else default_data_dir()

    endpoint, fresh = acquire(data_dir, args.port, args.token)

    # The carrier reads the port file, but printing the endpoint as JSON keeps
    # the CLI contract identical to the Rust binary (and helps manual runs).
    print(json.dumps(endpoint.as_dict()), flush=True)
    if not fresh:
        print("[datahub] reusing the running instance; exiting to keep a single writer", file=sys.stderr)
        return 0
    if args.resolve_only:
        release(data_dir)
        return 0

    market = MarketStore(data_dir / "market.duckdb")
    migrated = market.migrate_legacy_bars()
    if migrated:
        print(f"[datahub] migrated {migrated} legacy bars into ohlcv", file=sys.stderr)

    meta = MetaStore(data_dir / "meta.sqlite")
    registry = Registry(meta)
    sync = SyncService(market, meta, registry)

    app = create_app(
        AppState(
            market=market,
            meta=meta,
            registry=registry,
            sync=sync,
            token=endpoint.token,
            data_dir=str(data_dir),
        )
    )

    print(
        f"[datahub] python service listening on 127.0.0.1:{endpoint.port} (data_dir={data_dir})",
        file=sys.stderr,
        flush=True,
    )
    try:
        uvicorn.run(
            app,
            host="127.0.0.1",
            port=endpoint.port,
            log_level=os.environ.get("QUANTDSH_LOG", "info"),
            access_log=False,
        )
    finally:
        release(data_dir)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
