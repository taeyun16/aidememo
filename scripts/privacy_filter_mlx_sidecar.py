#!/usr/bin/env python3
"""MLX OpenAI Privacy Filter sidecar for AideMemo.

This is an Apple Silicon path for the Hugging Face MLX conversions such as
`mlx-community/openai-privacy-filter-mxfp4`. It exposes the same minimal API as
`privacy_filter_sidecar.py`:

  GET  /health
  POST /filter {"text": "..."} -> {"detected_spans": [...], ...}

Install the runtime from the current source package until PyPI includes
`mlx-embeddings` 0.1.1:

  python -m pip install git+https://github.com/Blaizzy/mlx-embeddings.git
"""

from __future__ import annotations

import argparse
import json
import math
from dataclasses import dataclass
from http.server import BaseHTTPRequestHandler, HTTPServer
from pathlib import Path
from typing import Any, Iterable

import mlx.core as mx


@dataclass(frozen=True)
class LabelInfo:
    token_to_span_label: dict[int, int]
    token_boundary_tags: dict[int, str | None]
    span_class_names: list[str]
    background_token_label: int
    background_span_label: int


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--host", default="127.0.0.1")
    parser.add_argument("--port", type=int, default=8091)
    parser.add_argument(
        "--model-dir",
        required=True,
        help="Local MLX model directory, e.g. /private/tmp/openai-privacy-filter-mlx-mxfp4.",
    )
    return parser.parse_args()


def load_model(model_dir: str) -> tuple[Any, Any]:
    try:
        from mlx_embeddings.utils import load
    except Exception as exc:  # pragma: no cover - optional runtime.
        raise SystemExit(
            "Could not import `mlx_embeddings`. Install the current MLX runtime:\n"
            "  python -m pip install git+https://github.com/Blaizzy/mlx-embeddings.git"
        ) from exc
    try:
        return load(str(Path(model_dir)))
    except Exception as exc:  # pragma: no cover - optional runtime/model failures.
        raise SystemExit(
            "Failed to load the MLX privacy-filter model. If the error mentions "
            "`openai_privacy_filter`, install mlx-embeddings from GitHub main; "
            "PyPI 0.1.0 does not support this architecture."
        ) from exc


def build_label_info(id2label: dict[str, str]) -> LabelInfo:
    span_class_names = ["O"]
    span_label_lookup = {"O": 0}
    token_to_span_label: dict[int, int] = {}
    token_boundary_tags: dict[int, str | None] = {}
    background_token_label = 0
    for raw_idx, name in id2label.items():
        idx = int(raw_idx)
        if name == "O":
            background_token_label = idx
            token_to_span_label[idx] = 0
            token_boundary_tags[idx] = None
            continue
        boundary, label = name.split("-", 1)
        if label not in span_label_lookup:
            span_label_lookup[label] = len(span_class_names)
            span_class_names.append(label)
        token_to_span_label[idx] = span_label_lookup[label]
        token_boundary_tags[idx] = boundary
    return LabelInfo(
        token_to_span_label=token_to_span_label,
        token_boundary_tags=token_boundary_tags,
        span_class_names=span_class_names,
        background_token_label=background_token_label,
        background_span_label=0,
    )


def valid_transition(prev: int, nxt: int, info: LabelInfo) -> bool:
    prev_span = info.token_to_span_label.get(prev)
    next_span = info.token_to_span_label.get(nxt)
    prev_tag = info.token_boundary_tags.get(prev)
    next_tag = info.token_boundary_tags.get(nxt)
    next_is_bg = next_span == info.background_span_label or nxt == info.background_token_label
    if prev_span is None or prev_tag is None:
        return next_is_bg or next_tag in {"B", "S"}
    if prev_span == info.background_span_label:
        return next_is_bg or next_tag in {"B", "S"}
    if prev_tag in {"E", "S"}:
        return next_is_bg or next_tag in {"B", "S"}
    if prev_tag in {"B", "I"}:
        return prev_span == next_span and next_tag in {"I", "E"}
    return False


def log_softmax_rows(logits: Any) -> list[list[float]]:
    rows = logits.tolist()
    output = []
    for row in rows:
        max_value = max(row)
        denom = math.log(sum(math.exp(value - max_value) for value in row))
        output.append([value - max_value - denom for value in row])
    return output


def viterbi(logprobs: list[list[float]], info: LabelInfo) -> list[int]:
    if not logprobs:
        return []
    n_classes = len(logprobs[0])
    start_ok = [
        idx == info.background_token_label or info.token_boundary_tags.get(idx) in {"B", "S"}
        for idx in range(n_classes)
    ]
    end_ok = [
        idx == info.background_token_label or info.token_boundary_tags.get(idx) in {"E", "S"}
        for idx in range(n_classes)
    ]
    scores = [lp if start_ok[idx] else -1e18 for idx, lp in enumerate(logprobs[0])]
    backptrs: list[list[int]] = []
    for row in logprobs[1:]:
        next_scores = [-1e18] * n_classes
        backs = [0] * n_classes
        for nxt in range(n_classes):
            best_score = -1e18
            best_prev = 0
            for prev in range(n_classes):
                if not valid_transition(prev, nxt, info):
                    continue
                score = scores[prev]
                if score > best_score:
                    best_score = score
                    best_prev = prev
            next_scores[nxt] = best_score + row[nxt]
            backs[nxt] = best_prev
        scores = next_scores
        backptrs.append(backs)
    scores = [score if end_ok[idx] else -1e18 for idx, score in enumerate(scores)]
    if not any(math.isfinite(score) and score > -1e17 for score in scores):
        return [max(range(len(row)), key=lambda idx: row[idx]) for row in logprobs]
    last = max(range(n_classes), key=lambda idx: scores[idx])
    path = [last]
    for backs in reversed(backptrs):
        last = backs[last]
        path.append(last)
    path.reverse()
    return path


def labels_to_spans(
    labels: list[int],
    offsets: list[tuple[int, int]],
    text: str,
    info: LabelInfo,
) -> list[dict[str, Any]]:
    spans = []
    current_label = None
    start_idx = None
    previous_idx = None
    for token_idx, label_id in enumerate(labels):
        span_label = info.token_to_span_label.get(label_id)
        boundary_tag = info.token_boundary_tags.get(label_id)
        if previous_idx is not None and token_idx != previous_idx + 1:
            if current_label is not None and start_idx is not None:
                spans.append((current_label, start_idx, previous_idx + 1))
            current_label = None
            start_idx = None
        if span_label is None or span_label == info.background_span_label:
            if current_label is not None and start_idx is not None:
                spans.append((current_label, start_idx, token_idx))
            current_label = None
            start_idx = None
            previous_idx = token_idx
            continue
        if boundary_tag == "S":
            if current_label is not None and start_idx is not None and previous_idx is not None:
                spans.append((current_label, start_idx, previous_idx + 1))
            spans.append((span_label, token_idx, token_idx + 1))
            current_label = None
            start_idx = None
        elif boundary_tag == "B":
            if current_label is not None and start_idx is not None and previous_idx is not None:
                spans.append((current_label, start_idx, previous_idx + 1))
            current_label = span_label
            start_idx = token_idx
        elif boundary_tag == "I":
            if current_label is None or current_label != span_label:
                if current_label is not None and start_idx is not None and previous_idx is not None:
                    spans.append((current_label, start_idx, previous_idx + 1))
                current_label = span_label
                start_idx = token_idx
        elif boundary_tag == "E":
            if current_label is None or current_label != span_label or start_idx is None:
                if current_label is not None and start_idx is not None and previous_idx is not None:
                    spans.append((current_label, start_idx, previous_idx + 1))
                spans.append((span_label, token_idx, token_idx + 1))
                current_label = None
                start_idx = None
            else:
                spans.append((current_label, start_idx, token_idx + 1))
                current_label = None
                start_idx = None
        previous_idx = token_idx
    if current_label is not None and start_idx is not None and previous_idx is not None:
        spans.append((current_label, start_idx, previous_idx + 1))
    return token_spans_to_char_spans(spans, offsets, text, info)


def token_spans_to_char_spans(
    spans: Iterable[tuple[int, int, int]],
    offsets: list[tuple[int, int]],
    text: str,
    info: LabelInfo,
) -> list[dict[str, Any]]:
    converted = []
    for label_idx, token_start, token_end in spans:
        if not (0 <= token_start < token_end <= len(offsets)):
            continue
        start = offsets[token_start][0]
        end = offsets[token_end - 1][1]
        while start < end and text[start].isspace():
            start += 1
        while end > start and text[end - 1].isspace():
            end -= 1
        if end <= start:
            continue
        label = info.span_class_names[label_idx]
        converted.append(
            {
                "label": label,
                "start": start,
                "end": end,
                "text": text[start:end],
                "placeholder": placeholder(label),
            }
        )
    return converted


def detect_spans(model: Any, tokenizer: Any, info: LabelInfo, text: str) -> list[dict[str, Any]]:
    encoded = tokenizer(text, return_tensors="mlx", return_offsets_mapping=True)
    outputs = model(encoded["input_ids"], attention_mask=encoded.get("attention_mask"))
    mx.eval(outputs.logits)
    offsets = [(int(start), int(end)) for start, end in encoded["offset_mapping"][0]]
    labels = viterbi(log_softmax_rows(outputs.logits[0]), info)
    return labels_to_spans(labels, offsets, text, info)


def placeholder(label: str) -> str:
    return f"<{label.upper()}>"


def redact_text(text: str, spans: list[dict[str, Any]]) -> str:
    pieces = []
    cursor = 0
    for span in sorted(spans, key=lambda item: (int(item["start"]), int(item["end"]))):
        start = int(span["start"])
        end = int(span["end"])
        if start < cursor:
            continue
        pieces.append(text[cursor:start])
        pieces.append(placeholder(str(span["label"])))
        cursor = end
    pieces.append(text[cursor:])
    return "".join(pieces)


def make_handler(model: Any, tokenizer: Any, info: LabelInfo) -> type[BaseHTTPRequestHandler]:
    class Handler(BaseHTTPRequestHandler):
        server_version = "aidememo-privacy-filter-mlx-sidecar/0.1"

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
                spans = detect_spans(model, tokenizer, info, text)
                by_label: dict[str, int] = {}
                for span in spans:
                    label = str(span["label"])
                    by_label[label] = by_label.get(label, 0) + 1
                self._json(
                    200,
                    {
                        "schema_version": 1,
                        "summary": {
                            "output_mode": "typed",
                            "span_count": len(spans),
                            "by_label": by_label,
                            "decoded_mismatch": False,
                        },
                        "text": text,
                        "detected_spans": spans,
                        "redacted_text": redact_text(text, spans),
                    },
                )
            except Exception as exc:  # pragma: no cover - runtime/model errors.
                self._json(500, {"error": str(exc)})

    return Handler


def main() -> None:
    args = parse_args()
    print(f"loading MLX privacy filter from {args.model_dir}...", flush=True)
    model, tokenizer = load_model(args.model_dir)
    info = build_label_info(model.config.id2label)
    server = HTTPServer((args.host, args.port), make_handler(model, tokenizer, info))
    print(
        f"mlx privacy filter sidecar listening on http://{args.host}:{args.port}",
        flush=True,
    )
    server.serve_forever()


if __name__ == "__main__":
    main()
