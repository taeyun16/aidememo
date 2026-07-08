#!/usr/bin/env python3
"""OpenAI Privacy Filter local sidecar for AideMemo.

This wraps the `opf` package from https://github.com/openai/privacy-filter and
exposes a tiny HTTP API:

  POST /filter {"text": "..."} -> OPF JSON output

The Rust core applies AideMemo's redact/block policy itself, so the sidecar only
needs to return typed spans.
"""

from __future__ import annotations

import argparse
import json
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from typing import Any


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--host", default="127.0.0.1")
    parser.add_argument("--port", type=int, default=8090)
    parser.add_argument(
        "--checkpoint",
        default=None,
        help="OPF checkpoint directory. Defaults to OPF_CHECKPOINT or ~/.opf/privacy_filter.",
    )
    parser.add_argument(
        "--device",
        default="cpu",
        help="OPF device name. Use cpu for portable local sidecars.",
    )
    parser.add_argument(
        "--decode-mode",
        default="viterbi",
        choices=["viterbi", "argmax"],
    )
    return parser.parse_args()


def load_redactor(args: argparse.Namespace) -> Any:
    try:
        from opf import OPF
    except Exception as exc:  # pragma: no cover - depends on optional package.
        raise SystemExit(
            "Could not import `opf`. Install OpenAI Privacy Filter first:\n"
            "  python -m pip install git+https://github.com/openai/privacy-filter.git"
        ) from exc

    return OPF(
        model=args.checkpoint,
        device=args.device,
        output_mode="typed",
        output_text_only=False,
        decode_mode=args.decode_mode,
    )


def make_handler(redactor: Any) -> type[BaseHTTPRequestHandler]:
    class Handler(BaseHTTPRequestHandler):
        server_version = "aidememo-privacy-filter-sidecar/0.1"

        def log_message(self, fmt: str, *args: Any) -> None:
            return

        def _json(self, code: int, payload: dict[str, Any]) -> None:
            raw = json.dumps(payload, ensure_ascii=False).encode("utf-8")
            self.send_response(code)
            self.send_header("Content-Type", "application/json; charset=utf-8")
            self.send_header("Content-Length", str(len(raw)))
            self.end_headers()
            self.wfile.write(raw)

        def do_GET(self) -> None:
            if self.path == "/health":
                self._json(200, {"ok": True})
                return
            self._json(404, {"error": "not found"})

        def do_POST(self) -> None:
            if self.path not in {"/filter", "/redact"}:
                self._json(404, {"error": "not found"})
                return
            try:
                length = int(self.headers.get("Content-Length", "0"))
                body = self.rfile.read(length)
                request = json.loads(body.decode("utf-8")) if body else {}
                text = request.get("text")
                if not isinstance(text, str):
                    self._json(400, {"error": "text string required"})
                    return
                result = redactor.redact(text)
                payload = result.to_dict() if hasattr(result, "to_dict") else result
                self._json(200, payload)
            except Exception as exc:  # pragma: no cover - runtime/model errors.
                self._json(500, {"error": str(exc)})

    return Handler


def main() -> None:
    args = parse_args()
    print("loading OpenAI Privacy Filter checkpoint...", flush=True)
    redactor = load_redactor(args)
    server = ThreadingHTTPServer((args.host, args.port), make_handler(redactor))
    print(f"privacy filter sidecar listening on http://{args.host}:{args.port}", flush=True)
    server.serve_forever()


if __name__ == "__main__":
    main()
