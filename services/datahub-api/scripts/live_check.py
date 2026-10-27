"""Live check against a real venue (network required).

Prints what the registry resolved, the capability matrix, and a small slice of
1m bars. Used during integration; not part of the smoke test.

    python scripts/live_check.py [source_id] [symbol] [interval]
"""

from __future__ import annotations

import asyncio
import shutil
import sys
import tempfile
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from app.db import MarketStore  # noqa: E402
from app.domain import now_ms  # noqa: E402
from app.meta import MetaStore  # noqa: E402
from app.providers import Registry  # noqa: E402
from app.providers.base import OhlcvRequest  # noqa: E402


async def main() -> int:
    source_id = sys.argv[1] if len(sys.argv) > 1 else "binance"
    symbol = sys.argv[2] if len(sys.argv) > 2 else "BTC/USDT"
    interval = sys.argv[3] if len(sys.argv) > 3 else "1m"

    data_dir = Path(tempfile.mkdtemp(prefix="quantdsh-live-"))
    market = MarketStore(data_dir / "market.duckdb")
    meta = MetaStore(data_dir / "meta.sqlite")
    registry = Registry(meta)
    try:
        row = meta.get_source(source_id)
        print("source:", {k: row[k] for k in ("id", "provider", "venue", "inst_types", "base_urls", "config")})
        provider = registry.provider(source_id)

        started = time.perf_counter()
        caps = await provider.capabilities("spot")
        print(f"spot capabilities ({time.perf_counter() - started:.1f}s):", caps.as_api())

        until = now_ms()
        since = until - 2 * 86_400_000
        started = time.perf_counter()
        bars = await provider.fetch_ohlcv(
            OhlcvRequest(venue=provider.cfg.venue or source_id, symbol=symbol, inst_type="spot",
                         interval=interval, since=since, until=until, limit=1000)
        )
        print(f"fetched {len(bars)} bars in {time.perf_counter() - started:.1f}s")
        if bars:
            for bar in (bars[0], bars[-1]):
                print("  ", bar.ts, bar.open, bar.high, bar.low, bar.close, bar.volume, bar.closed, bar.source)

        print("instruments (spot, first 5):")
        for inst in (await provider.instruments("spot"))[:5]:
            print("  ", inst["instrument_id"], inst["venue_symbol"], inst["contract_size"])
        await provider.aclose()
    finally:
        market.close()
        meta.close()
        shutil.rmtree(data_dir, ignore_errors=True)
    return 0


if __name__ == "__main__":
    raise SystemExit(asyncio.run(main()))
