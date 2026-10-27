"""Start the data service with the *bundled* interpreter and probe it.

This is the one check a wheel-level import test cannot make: the carrier spawns
`<resource>/runtime/python/python.exe -s -B -m app --data-dir <dir>` with the
backend as cwd, from a machine that has no Python on PATH. This script does the
same thing, waits for `datahub.json`, and asserts the HTTP contract.

    python scripts/verify_bundled_runtime.py [--python <exe>] [--keep]

Exit code 0 means the shipped runtime can serve the workbench.
"""

from __future__ import annotations

import argparse
import json
import os
import shutil
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
DEFAULT_PYTHON = ROOT / "build" / "python-runtime" / "python" / ("python.exe" if os.name == "nt" else "bin/python3")
BACKEND = ROOT / "services" / "datahub-api"


def request(url: str, token: str | None = None, timeout: float = 10.0) -> tuple[int, str]:
    req = urllib.request.Request(url)
    if token:
        req.add_header("Authorization", f"Bearer {token}")
    try:
        with urllib.request.urlopen(req, timeout=timeout) as response:
            return response.status, response.read().decode("utf-8")
    except urllib.error.HTTPError as error:
        return error.code, error.read().decode("utf-8")


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--python", default=str(DEFAULT_PYTHON))
    parser.add_argument(
        "--backend",
        default=str(BACKEND),
        help="directory containing the `app` package (default: the source tree; "
        "point it at a staged copy to prove the service needs nothing outside `app/`)",
    )
    parser.add_argument("--keep", action="store_true", help="keep the temp data dir")
    args = parser.parse_args()

    python = Path(args.python)
    backend = Path(args.backend)
    if not python.exists():
        print(f"FAIL  bundled interpreter missing: {python}")
        return 1
    if not (backend / "app" / "__main__.py").exists():
        print(f"FAIL  no app package under {backend}")
        return 1

    data_dir = Path(tempfile.mkdtemp(prefix="quantdsh-bundled-"))
    log = open(data_dir / "stderr.log", "w", encoding="utf-8")
    # `-s` is what makes the check meaningful: user site-packages are ignored, so
    # every import must come from the bundled tree.
    started = time.monotonic()
    child = subprocess.Popen(
        [str(python), "-s", "-B", "-m", "app", "--data-dir", str(data_dir)],
        cwd=str(backend),
        stdin=subprocess.DEVNULL,
        stdout=log,
        stderr=log,
        env={**os.environ, "PYTHONUTF8": "1", "PYTHONUNBUFFERED": "1"},
    )

    port_file = data_dir / "datahub.json"
    endpoint: dict | None = None
    try:
        deadline = time.monotonic() + 90
        while time.monotonic() < deadline:
            if port_file.exists():
                try:
                    endpoint = json.loads(port_file.read_text(encoding="utf-8"))
                    break
                except json.JSONDecodeError:
                    pass
            if child.poll() is not None:
                break
            time.sleep(0.1)

        failures: list[str] = []

        def check(name: str, ok: bool, detail: object = "") -> None:
            print(f"{'PASS' if ok else 'FAIL'}  {name}{'' if ok else f'  -> {detail}'}")
            if not ok:
                failures.append(name)

        if endpoint is None:
            log.flush()
            tail = (data_dir / "stderr.log").read_text(encoding="utf-8", errors="replace")[-2000:]
            print(f"FAIL  port file never appeared (exit={child.poll()}, {time.monotonic() - started:.1f}s)")
            print(tail)
            return 1

        elapsed = time.monotonic() - started
        print(f"[runtime] port file after {elapsed:.1f}s  pid={endpoint.get('pid')}  port={endpoint['port']}")

        base = f"http://127.0.0.1:{endpoint['port']}"
        token = endpoint["token"]

        status, body = request(f"{base}/health")
        check("health without token", status == 200, body[:200])

        status, _ = request(f"{base}/api/sources")
        check("api rejects missing token", status == 401, status)

        status, body = request(f"{base}/api/sources", token)
        ids = json.loads(body).get("sources", []) if status == 200 else []
        check("sources seeded", status == 200 and {"binance", "okx", "bybit", "replay"} <= set(ids), ids)

        status, body = request(f"{base}/api/schema", token)
        check("schema reachable", status == 200 and "ohlcv" in body, status)

        check("startup under 60s", elapsed < 60, f"{elapsed:.1f}s")

        print("\nall checks passed" if not failures else f"\n{len(failures)} check(s) failed")
        return 0 if not failures else 1
    finally:
        if child.poll() is None:
            child.terminate()
            try:
                child.wait(timeout=10)
            except subprocess.TimeoutExpired:
                child.kill()
        log.close()
        if not args.keep:
            shutil.rmtree(data_dir, ignore_errors=True)


if __name__ == "__main__":
    sys.exit(main())
