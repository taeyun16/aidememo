#!/usr/bin/env python3
"""Serve an MLX LFM embedding model behind a TEI-compatible HTTP surface.

This is the experimental bridge between AideMemo's Rust embedding-provider
abstraction and Mac-local LFM MLX models. It intentionally speaks the same
minimal endpoints AideMemo's `model.provider = "tei"` path already knows:

  GET  /health
  GET  /info
  POST /embed      {"inputs": "text"} or {"inputs": ["text", ...]}

Recommended AideMemo config:

  aidememo config set model.provider lfm-sidecar
  aidememo config set model.endpoint http://127.0.0.1:8088
  aidememo config set model.name mlx-community/LFM2.5-Embedding-350M-4bit
  aidememo config set model.dimension 1024
  aidememo config set model.query_prefix "query: "
  aidememo config set model.document_prefix "document: "

The sidecar itself does not add query/document prompts by default because
AideMemo already applies `model.query_prefix` and `model.document_prefix` before
calling the provider. Pass `--apply-role-prefixes` only for non-AideMemo clients
that send raw text plus `?role=query|document`.
"""

from __future__ import annotations

import argparse
import json
import signal
import sys
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from typing import Any
from urllib.parse import parse_qs, urlparse

from lfm_mlx_dense_eval import MlxEmbedder


class SidecarState:
    def __init__(
        self,
        *,
        model_dir: Path,
        model_id: str,
        batch_size: int,
        apply_role_prefixes: bool,
    ) -> None:
        started = time.perf_counter()
        self.embedder = MlxEmbedder(model_dir)
        # AideMemo sends already-prefixed text, so default to raw encoding.
        if not apply_role_prefixes:
            self.embedder.query_prefix = ""
            self.embedder.document_prefix = ""
        self.model_dir = model_dir
        self.model_id = model_id
        self.batch_size = batch_size
        self.apply_role_prefixes = apply_role_prefixes
        probe = self.embedder.encode(["."], role="document", batch_size=1)
        self.dimension = int(probe.shape[1])
        self.model_load_ms = round((time.perf_counter() - started) * 1000, 2)

    def embed(self, texts: list[str], role: str) -> list[list[float]]:
        encoded = self.embedder.encode(texts, role=role, batch_size=self.batch_size)
        return encoded.astype("float32").tolist()


class Handler(BaseHTTPRequestHandler):
    server_version = "aidememo-lfm-mlx-sidecar/0.1"

    def log_message(self, fmt: str, *args: Any) -> None:
        if getattr(self.server, "quiet", False):
            return
        super().log_message(fmt, *args)

    @property
    def state(self) -> SidecarState:
        return getattr(self.server, "state")

    def send_json(self, payload: Any, status: int = 200) -> None:
        body = json.dumps(payload, ensure_ascii=False).encode("utf-8")
        self.send_response(status)
        self.send_header("Content-Type", "application/json; charset=utf-8")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def read_json(self) -> dict[str, Any]:
        length = int(self.headers.get("Content-Length", "0"))
        if length <= 0:
            return {}
        return json.loads(self.rfile.read(length).decode("utf-8"))

    def do_GET(self) -> None:  # noqa: N802
        parsed = urlparse(self.path)
        if parsed.path == "/health":
            self.send_json({"ok": True, "backend": self.server_version})
            return
        if parsed.path == "/info":
            self.send_json(
                {
                    "model_id": self.state.model_id,
                    "dimension": self.state.dimension,
                    "max_client_batch_size": self.state.batch_size,
                    "model_load_ms": self.state.model_load_ms,
                    "backend": "mlx-lfm-embedding-sidecar",
                    "model_dir": str(self.state.model_dir),
                    "client_prefixes_expected": not self.state.apply_role_prefixes,
                }
            )
            return
        self.send_json({"error": f"unknown endpoint {parsed.path}"}, status=404)

    def do_POST(self) -> None:  # noqa: N802
        parsed = urlparse(self.path)
        if parsed.path != "/embed":
            self.send_json({"error": f"unknown endpoint {parsed.path}"}, status=404)
            return
        try:
            body = self.read_json()
            raw_inputs = body.get("inputs", body.get("input"))
            if isinstance(raw_inputs, str):
                texts = [raw_inputs]
            elif isinstance(raw_inputs, list):
                texts = [str(item) for item in raw_inputs]
            else:
                raise ValueError("body must contain inputs as string or list")
            if not texts:
                self.send_json([])
                return
            params = parse_qs(parsed.query)
            role = str(body.get("role") or params.get("role", ["document"])[0])
            if role not in {"query", "document"}:
                raise ValueError("role must be query or document")
            vectors = self.state.embed(texts, role=role)
            self.send_json(vectors)
        except Exception as exc:  # pragma: no cover - exercised by manual server use
            self.send_json({"error": str(exc)}, status=400)


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--model-dir", required=True, type=Path)
    parser.add_argument("--host", default="127.0.0.1")
    parser.add_argument("--port", type=int, default=8088)
    parser.add_argument("--batch-size", type=int, default=8)
    parser.add_argument("--model-id", default="mlx-community/LFM2.5-Embedding-350M-4bit")
    parser.add_argument(
        "--apply-role-prefixes",
        action="store_true",
        help="Apply the model repo's query/document prompts inside the sidecar.",
    )
    parser.add_argument("--quiet", action="store_true")
    args = parser.parse_args()

    state = SidecarState(
        model_dir=args.model_dir,
        model_id=args.model_id,
        batch_size=args.batch_size,
        apply_role_prefixes=args.apply_role_prefixes,
    )
    server = ThreadingHTTPServer((args.host, args.port), Handler)
    server.state = state  # type: ignore[attr-defined]
    server.quiet = args.quiet  # type: ignore[attr-defined]

    def stop(_signum: int, _frame: Any) -> None:
        server.shutdown()

    signal.signal(signal.SIGTERM, stop)
    signal.signal(signal.SIGINT, stop)

    print(
        json.dumps(
            {
                "url": f"http://{args.host}:{args.port}",
                "model_id": state.model_id,
                "dimension": state.dimension,
                "model_load_ms": state.model_load_ms,
                "client_prefixes_expected": not state.apply_role_prefixes,
            },
            ensure_ascii=False,
        ),
        flush=True,
    )
    try:
        server.serve_forever()
    finally:
        server.server_close()


if __name__ == "__main__":
    try:
        main()
    except BrokenPipeError:
        sys.exit(1)
