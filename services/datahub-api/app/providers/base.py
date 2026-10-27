"""Provider contract.

A provider turns one row of the `sources` table into bars. It owns everything
venue-specific (symbol spelling, page size, ordering, contract size); the sync
engine above it never branches on the exchange name.
"""

from __future__ import annotations

from abc import ABC, abstractmethod
from dataclasses import dataclass, field
from typing import Any

from ..domain import Bar, Capabilities


class ProviderError(RuntimeError):
    """Anything the venue said that we cannot turn into bars."""


class Unsupported(ProviderError):
    """The combination is structurally unavailable (not 'empty today').

    Raised instead of returning `[]`: an empty list would be recorded as real
    missing data and turn into a gap finding that can never be closed.
    """


@dataclass
class OhlcvRequest:
    venue: str
    symbol: str  # unified `BTC/USDT`
    inst_type: str  # spot | swap
    interval: str
    since: int
    until: int
    limit: int = 1000
    settle: str | None = None


@dataclass
class SourceConfig:
    id: str
    provider: str
    venue: str | None
    base_urls: list[str] = field(default_factory=list)
    inst_types: list[str] = field(default_factory=lambda: ["spot"])
    intervals: list[str] | None = None
    auth_kind: str = "none"
    auth_secret_ref: str | None = None
    config: dict[str, Any] = field(default_factory=dict)
    priority: int = 100

    @classmethod
    def from_row(cls, row: dict[str, Any]) -> "SourceConfig":
        return cls(
            id=row["id"],
            provider=row["provider"],
            venue=row.get("venue"),
            base_urls=list(row.get("base_urls") or []),
            inst_types=list(row.get("inst_types") or ["spot"]),
            intervals=row.get("intervals"),
            auth_kind=row.get("auth_kind") or "none",
            auth_secret_ref=row.get("auth_secret_ref"),
            config=dict(row.get("config") or {}),
            priority=int(row.get("priority") or 100),
        )


class Provider(ABC):
    kind: str = "base"

    def __init__(self, cfg: SourceConfig) -> None:
        self.cfg = cfg

    @property
    def id(self) -> str:
        return self.cfg.id

    @abstractmethod
    async def capabilities(self, inst_type: str = "spot") -> Capabilities:
        ...

    @abstractmethod
    async def instruments(self, inst_type: str = "spot") -> list[dict[str, Any]]:
        ...

    @abstractmethod
    async def fetch_ohlcv(self, req: OhlcvRequest) -> list[Bar]:
        ...

    async def aclose(self) -> None:
        return None
