"""QuantDSH data service (FastAPI).

Replaces the former Rust `quantdsh-datahub` sidecar with the same HTTP contract
so the shipped webview keeps working: `/health` is unauthenticated, everything
under `/api/*` requires `Authorization: Bearer <token>` from the port file.
"""

__all__ = ["__version__"]

__version__ = "2.0.0"
