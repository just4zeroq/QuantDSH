"""Provider registry: `sources` rows in, live providers out.

Cached per source id because a ccxt client holds loaded markets and a rate
limiter; rebuilding it per request would throw that away and get us throttled.
"""

from __future__ import annotations

from typing import Any

from .base import Provider, SourceConfig, Unsupported
from .ccxt_source import CcxtProvider
from .official import OfficialProvider
from .replay import ReplayProvider

_PROVIDER_TYPES: dict[str, type[Provider]] = {
    "ccxt": CcxtProvider,
    "official": OfficialProvider,
    "replay": ReplayProvider,
}


class Registry:
    def __init__(self, meta) -> None:
        self._meta = meta
        self._cache: dict[str, Provider] = {}

    def provider(self, source_id: str) -> Provider:
        cached = self._cache.get(source_id)
        if cached is not None:
            return cached
        row = self._meta.get_source(source_id)
        if row is None:
            raise Unsupported(f"未知数据源: {source_id}")
        if not row.get("enabled"):
            raise Unsupported(f"数据源 {source_id} 已停用")
        klass = _PROVIDER_TYPES.get(row["provider"])
        if klass is None:
            raise Unsupported(f"数据源 {source_id} 的 provider 类型 {row['provider']} 未实现")
        provider = klass(SourceConfig.from_row(row))
        self._cache[source_id] = provider
        return provider

    def invalidate(self, source_id: str | None = None) -> None:
        if source_id is None:
            self._cache.clear()
            return
        self._cache.pop(source_id, None)

    def describe(self, source_id: str) -> dict[str, Any]:
        row = self._meta.get_source(source_id)
        if row is None:
            raise Unsupported(f"未知数据源: {source_id}")
        return row

    async def aclose(self) -> None:
        for provider in self._cache.values():
            try:
                await provider.aclose()
            except Exception:  # noqa: BLE001 - shutdown must not raise
                pass
        self._cache.clear()
