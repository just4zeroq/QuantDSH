"""Quality checks for OHLCV.

Four checks ship in v1 — they are the ones that turn "we have rows" into "the
rows are usable": structural gaps, OHLC contradictions, stale tails and
duplicate timestamps inside one batch.

Two rules are non-negotiable:

* **Report, never repair.** Interpolating a gap manufactures data that looks
  complete and is wrong, which is worse than a visible hole.
* **NULL is not zero.** Missing fields are skipped, never coerced, otherwise
  every average and ratio silently drifts.
"""

from __future__ import annotations

from typing import Any

from .domain import DEFAULT_NAMESPACE, DEFAULT_PRICE_TYPE, Bar, interval_ms, now_ms


def check_bars(
    store,
    *,
    instrument_id: str,
    interval: str,
    namespace: str = DEFAULT_NAMESPACE,
    price_type: str = DEFAULT_PRICE_TYPE,
    jump_threshold: float = 0.2,
) -> dict[str, Any]:
    rows = store.query(
        'SELECT ts, open, high, low, close, volume, closed FROM ohlcv '
        'WHERE namespace = ? AND instrument_id = ? AND "interval" = ? AND price_type = ? ORDER BY ts',
        [namespace, instrument_id, interval, price_type],
    )
    step = interval_ms(interval)
    findings: list[dict[str, Any]] = []

    if not rows:
        return {"findings": [], "passed": True, "rows": 0}

    first, last = rows[0]["ts"], rows[-1]["ts"]

    # gap — contiguous missing points on the interval grid
    gaps: list[dict[str, int]] = []
    for prev, cur in zip(rows, rows[1:]):
        delta = cur["ts"] - prev["ts"]
        if delta > step:
            missing = delta // step - 1
            gaps.append({"from": prev["ts"] + step, "to": cur["ts"] - step, "missing": int(missing)})
    if gaps:
        findings.append(
            {
                "code": "gap",
                "severity": "warn",
                "itemCount": sum(g["missing"] for g in gaps),
                "scopeFrom": first,
                "scopeTo": last,
                "detail": {"segments": gaps[:50], "segmentsTotal": len(gaps)},
            }
        )

    # ohlc_violation — the bar contradicts itself
    bad = [
        r
        for r in rows
        if r["high"] is not None
        and r["low"] is not None
        and r["open"] is not None
        and r["close"] is not None
        and (r["high"] < r["low"] or r["high"] < max(r["open"], r["close"]) or r["low"] > min(r["open"], r["close"]))
    ]
    if bad:
        findings.append(
            {
                "code": "ohlc_violation",
                "severity": "error",
                "itemCount": len(bad),
                "scopeFrom": bad[0]["ts"],
                "scopeTo": bad[-1]["ts"],
                "detail": {"sample": [{"ts": r["ts"], "open": r["open"], "high": r["high"], "low": r["low"], "close": r["close"]} for r in bad[:10]]},
            }
        )

    # null_value — required columns must exist
    nulls = [r for r in rows if any(r[c] is None for c in ("open", "high", "low", "close"))]
    if nulls:
        findings.append(
            {
                "code": "null_value",
                "severity": "error",
                "itemCount": len(nulls),
                "scopeFrom": nulls[0]["ts"],
                "scopeTo": nulls[-1]["ts"],
                "detail": {"sample": [r["ts"] for r in nulls[:20]]},
            }
        )

    # unclosed_bar — an unclosed bar in bronze will keep mutating
    unclosed = [r for r in rows if not r["closed"]]
    if unclosed:
        findings.append(
            {
                "code": "unclosed_bar",
                "severity": "warn",
                "itemCount": len(unclosed),
                "scopeFrom": unclosed[0]["ts"],
                "scopeTo": unclosed[-1]["ts"],
                "detail": {"sample": [r["ts"] for r in unclosed[:20]]},
            }
        )

    # stale — the tail stopped advancing
    now = now_ms()
    if last + 2 * step < now:
        findings.append(
            {
                "code": "stale",
                "severity": "error",
                "itemCount": 1,
                "scopeFrom": last,
                "scopeTo": last,
                "detail": {"lastTs": last, "ageMs": now - last, "intervalMs": step},
            }
        )

    # price_jump — a single-bar move beyond the threshold usually means a unit
    # or symbol mix-up rather than a real market event
    jumps = []
    for prev, cur in zip(rows, rows[1:]):
        if not prev["close"]:
            continue
        change = abs(cur["close"] - prev["close"]) / prev["close"]
        if change > jump_threshold:
            jumps.append({"ts": cur["ts"], "change": round(change, 6)})
    if jumps:
        findings.append(
            {
                "code": "price_jump",
                "severity": "warn",
                "itemCount": len(jumps),
                "scopeFrom": jumps[0]["ts"],
                "scopeTo": jumps[-1]["ts"],
                "detail": {"sample": jumps[:10], "threshold": jump_threshold},
            }
        )

    passed = not any(f["severity"] == "error" for f in findings)
    return {"findings": findings, "passed": passed, "rows": len(rows)}


def duplicates(bars: list[Bar]) -> int:
    seen = set()
    dupes = 0
    for bar in bars:
        key = (bar.namespace, bar.instrument_id, bar.interval, bar.price_type, bar.ts)
        if key in seen:
            dupes += 1
        seen.add(key)
    return dupes
