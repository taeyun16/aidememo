#!/usr/bin/env python3
"""Check or refresh source fingerprints for translated Docusaurus docs."""

from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path
import re
import sys


ROOT = Path(__file__).resolve().parents[1]
DOCS = ROOT / "docs"
SIDEBAR = ROOT / "website" / "sidebars.js"
KO_DOCS = ROOT / "website" / "i18n" / "ko" / "docusaurus-plugin-content-docs" / "current"
STATUS = ROOT / "website" / "i18n" / "ko" / "translation-status.json"


def source_digest(doc_id: str) -> str:
    return hashlib.sha256((DOCS / f"{doc_id}.md").read_bytes()).hexdigest()


def sidebar_doc_ids() -> set[str]:
    text = SIDEBAR.read_text(encoding="utf-8")
    return set(re.findall(r"['\"]([A-Z][A-Z0-9_]+)['\"]", text))


def load_status() -> dict[str, object]:
    return json.loads(STATUS.read_text(encoding="utf-8"))


def inline_code_tokens(text: str) -> set[str]:
    without_link_labels = re.sub(r"\[`[^`\n]+`\]\([^)]+\)", "", text)
    return set(re.findall(r"(?<!`)`([^`\n]+)`(?!`)", without_link_labels))


def validate(status: dict[str, object], *, check_hashes: bool) -> list[str]:
    errors: list[str] = []
    if status.get("locale") != "ko" or status.get("source_locale") != "en":
        errors.append("translation status must declare locale=ko and source_locale=en")

    translated = status.get("translated_docs")
    fallback = status.get("fallback_docs")
    token_parity = status.get("token_parity_docs")
    if not isinstance(translated, dict) or not all(
        isinstance(key, str) and isinstance(value, str) for key, value in translated.items()
    ):
        return [*errors, "translated_docs must be a doc-id to SHA-256 object"]
    if not isinstance(fallback, list) or not all(isinstance(item, str) for item in fallback):
        return [*errors, "fallback_docs must be a list of doc ids"]
    if not isinstance(token_parity, list) or not all(isinstance(item, str) for item in token_parity):
        return [*errors, "token_parity_docs must be a list of translated doc ids"]

    translated_ids = set(translated)
    fallback_ids = set(fallback)
    overlap = translated_ids & fallback_ids
    if overlap:
        errors.append(f"translated_docs and fallback_docs overlap: {sorted(overlap)}")

    expected_ids = sidebar_doc_ids()
    covered_ids = translated_ids | fallback_ids
    if covered_ids != expected_ids:
        errors.append(
            "translation coverage does not match the public sidebar: "
            f"missing={sorted(expected_ids - covered_ids)} extra={sorted(covered_ids - expected_ids)}"
        )

    token_parity_ids = set(token_parity)
    if not token_parity_ids <= translated_ids:
        errors.append(
            "token_parity_docs must be translated: "
            f"{sorted(token_parity_ids - translated_ids)}"
        )

    for doc_id, expected_digest in sorted(translated.items()):
        source = DOCS / f"{doc_id}.md"
        target = KO_DOCS / f"{doc_id}.md"
        if not source.exists():
            errors.append(f"translated source is missing: {source.relative_to(ROOT)}")
            continue
        if not target.exists():
            errors.append(f"Korean translation is missing: {target.relative_to(ROOT)}")
            continue
        target_text = target.read_text(encoding="utf-8")
        if not re.search(r"[가-힣]", target_text):
            errors.append(f"Korean translation contains no Hangul: {target.relative_to(ROOT)}")
        if check_hashes:
            actual_digest = source_digest(doc_id)
            if expected_digest != actual_digest:
                errors.append(
                    f"{doc_id} translation is stale: expected source SHA-256 {expected_digest}, "
                    f"current {actual_digest}; refresh the translation, then run "
                    "python3 scripts/docs-i18n-status.py update"
                )
        if doc_id in token_parity_ids:
            source_tokens = inline_code_tokens(source.read_text(encoding="utf-8"))
            target_tokens = inline_code_tokens(target_text)
            if source_tokens != target_tokens:
                errors.append(
                    f"{doc_id} inline code tokens drifted: "
                    f"missing={sorted(source_tokens - target_tokens)} "
                    f"extra={sorted(target_tokens - source_tokens)}"
                )

    for doc_id in sorted(fallback_ids):
        source = DOCS / f"{doc_id}.md"
        target = KO_DOCS / f"{doc_id}.md"
        if not source.exists():
            errors.append(f"fallback source is missing: {source.relative_to(ROOT)}")
        if target.exists():
            errors.append(
                f"{target.relative_to(ROOT)} exists but {doc_id} is still listed as an English fallback"
            )

    return errors


def self_test(status: dict[str, object]) -> list[str]:
    errors: list[str] = []
    stale = json.loads(json.dumps(status))
    stale_docs = stale.get("translated_docs")
    if not isinstance(stale_docs, dict) or not stale_docs:
        return ["self-test requires at least one translated doc"]
    first_doc = sorted(stale_docs)[0]
    stale_docs[first_doc] = "0" * 64
    if not any("translation is stale" in error for error in validate(stale, check_hashes=True)):
        errors.append("self-test did not reject a stale source fingerprint")

    incomplete = json.loads(json.dumps(status))
    fallback = incomplete.get("fallback_docs")
    if not isinstance(fallback, list) or not fallback:
        return [*errors, "self-test requires at least one fallback doc"]
    fallback.pop()
    if not any("coverage does not match" in error for error in validate(incomplete, check_hashes=False)):
        errors.append("self-test did not reject incomplete sidebar coverage")
    if inline_code_tokens("Use `aidememo search` and [`Guide`](GUIDE.md).") != {"aidememo search"}:
        errors.append("self-test did not exclude translated Markdown link labels from token parity")
    return errors


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "command",
        choices=("check", "update", "self-test"),
        nargs="?",
        default="check",
    )
    args = parser.parse_args()

    status = load_status()
    if args.command == "self-test":
        errors = self_test(status)
        if errors:
            print("docs i18n status self-test failed:", file=sys.stderr)
            for error in errors:
                print(f"- {error}", file=sys.stderr)
            return 1
        print("docs i18n status self-test passed")
        return 0

    errors = validate(status, check_hashes=args.command == "check")
    if args.command == "check":
        errors.extend(self_test(status))
    if errors:
        print("docs i18n status failed:", file=sys.stderr)
        for error in errors:
            print(f"- {error}", file=sys.stderr)
        return 1

    translated = status["translated_docs"]
    if args.command == "update":
        if not isinstance(translated, dict):
            raise TypeError("translated_docs was validated as a dictionary")
        status["translated_docs"] = {
            doc_id: source_digest(doc_id) for doc_id in sorted(translated)
        }
        STATUS.write_text(
            json.dumps(status, ensure_ascii=False, indent=2) + "\n",
            encoding="utf-8",
        )
        print(f"updated source fingerprints for {len(translated)} Korean docs")
    else:
        print(
            "docs i18n status passed: "
            f"{len(translated)} Korean docs, {len(status['fallback_docs'])} English fallbacks"
        )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
