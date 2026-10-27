"""Process configuration: data directory, port, token and the port-file contract.

The port file (`datahub.json`) is the interface between this service and the
Tauri carrier. It is written **before** the HTTP server starts listening so the
carrier can read it without racing the server, and it survives a carrier
restart — a second launch probes `/health` and exits instead of opening DuckDB
twice (DuckDB allows exactly one writer).
"""

from __future__ import annotations

import json
import os
import secrets
import socket
import time
from dataclasses import dataclass
from pathlib import Path

DEFAULT_PORT = 17821
PORT_SCAN = 20
PORTFILE_NAME = "datahub.json"


def default_data_dir() -> Path:
    """Same rule as the Rust daemon and the carrier, so all three agree."""
    override = os.environ.get("QUANTDSH_DATA_DIR")
    if override:
        return Path(override)
    base = os.environ.get("LOCALAPPDATA")
    if base:
        return Path(base) / "QuantDSH" / "data"
    return Path.home() / ".local" / "share" / "QuantDSH" / "data"


@dataclass
class Endpoint:
    port: int
    token: str
    pid: int
    started_at: int
    data_dir: str

    def as_dict(self) -> dict:
        return {
            "port": self.port,
            "token": self.token,
            "pid": self.pid,
            "started_at": self.started_at,
            "data_dir": self.data_dir,
        }


def generate_token() -> str:
    # 48 hex chars = 192 bits. The token is what separates "loopback only"
    # from "any local process can rewrite the market database".
    return secrets.token_hex(24)


def portfile_path(data_dir: Path) -> Path:
    return data_dir / PORTFILE_NAME


def read_endpoint(path: Path) -> Endpoint | None:
    try:
        raw = json.loads(path.read_text(encoding="utf-8"))
        return Endpoint(
            port=int(raw["port"]),
            token=str(raw["token"]),
            pid=int(raw.get("pid", 0)),
            started_at=int(raw.get("started_at", 0)),
            data_dir=str(raw.get("data_dir", "")),
        )
    except (OSError, ValueError, KeyError, TypeError):
        return None


def write_endpoint(path: Path, ep: Endpoint) -> None:
    path.write_text(json.dumps(ep.as_dict(), indent=2), encoding="utf-8")


def port_free(port: int) -> bool:
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as sock:
        try:
            sock.bind(("127.0.0.1", port))
        except OSError:
            return False
    return True


def probe(port: int, timeout: float = 1.0) -> bool:
    """Is a datahub already answering on this port?

    Uses a raw socket + minimal HTTP/1.1 request so we do not need an async
    client at import time; this runs before the event loop starts.
    """
    try:
        with socket.create_connection(("127.0.0.1", port), timeout=timeout) as sock:
            sock.sendall(b"GET /health HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n")
            head = sock.recv(64)
            return head.startswith(b"HTTP/1.1 200") or head.startswith(b"HTTP/1.0 200")
    except OSError:
        return False


def acquire(data_dir: Path, preferred_port: int, token: str | None) -> tuple[Endpoint, bool]:
    """Return `(endpoint, is_fresh)`.

    `is_fresh is False` means another live instance owns the database and this
    process must print the endpoint and exit.
    """
    data_dir.mkdir(parents=True, exist_ok=True)
    path = portfile_path(data_dir)

    existing = read_endpoint(path)
    if existing and probe(existing.port):
        return existing, False
    if existing:
        # Stale file: nothing answers on that port. Drop it and start clean.
        path.unlink(missing_ok=True)

    port = preferred_port
    for _ in range(PORT_SCAN):
        if port_free(port):
            break
        port += 1
    if not port_free(port):
        raise RuntimeError(f"no free port in {preferred_port}..{preferred_port + PORT_SCAN}")

    ep = Endpoint(
        port=port,
        token=token or generate_token(),
        pid=os.getpid(),
        started_at=int(time.time() * 1000),
        data_dir=str(data_dir),
    )
    write_endpoint(path, ep)
    return ep, True


def release(data_dir: Path) -> None:
    portfile_path(data_dir).unlink(missing_ok=True)
