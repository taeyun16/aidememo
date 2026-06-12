"""``hermes aidememo <subcommand>`` CLI subtree.

Hermes's ``register_cli_command`` takes a ``setup_fn`` that wires an
argparse subparser, plus a ``handler_fn`` that runs the chosen
subcommand. We mirror the most-used aidememo verbs as thin wrappers so
users can stay in the ``hermes`` invocation when scripting.
"""

from __future__ import annotations

import argparse
from typing import Any

from .client import CLIENT_ERRORS, AideMemoClient
from .tools import to_pretty_json


def _setup(parser: argparse.ArgumentParser) -> None:
    subs = parser.add_subparsers(dest="subcmd", required=True)

    p_query = subs.add_parser("query", help="Unified context fetch")
    p_query.add_argument("topic")
    p_query.add_argument("-l", "--limit", type=int, default=5)
    p_query.add_argument("-d", "--depth", type=int, default=2)

    p_search = subs.add_parser("search", help="Hybrid BM25 + semantic search")
    p_search.add_argument("query")
    p_search.add_argument("-l", "--limit", type=int, default=10)

    p_recent = subs.add_parser("recent", help="Recent facts")
    p_recent.add_argument("--last", default="7d")
    p_recent.add_argument("-n", "--limit", type=int, default=10)

    p_add = subs.add_parser("add", help="Add a fact")
    p_add.add_argument("content")
    p_add.add_argument("--entities", help="Comma-separated entity names")
    p_add.add_argument("--type", dest="fact_type", default="note")
    p_add.add_argument("--tag", action="append", default=[])

    subs.add_parser("stats", help="Counts + size")
    subs.add_parser("lint", help="Graph health check")


def _handler_factory(client: AideMemoClient):
    def _handler(args: argparse.Namespace) -> int:
        try:
            if args.subcmd == "query":
                print(to_pretty_json(client.query(args.topic, limit=args.limit, depth=args.depth)))
            elif args.subcmd == "search":
                print(to_pretty_json(client.search(args.query, limit=args.limit)))
            elif args.subcmd == "recent":
                print(to_pretty_json(client.recent(last=args.last, limit=args.limit)))
            elif args.subcmd == "add":
                ents = (
                    [s.strip() for s in args.entities.split(",") if s.strip()]
                    if args.entities
                    else None
                )
                fid = client.fact_add(
                    args.content, entities=ents, fact_type=args.fact_type, tags=args.tag
                )
                print(fid)
            elif args.subcmd == "stats":
                print(to_pretty_json(client.stats()))
            elif args.subcmd == "lint":
                print(to_pretty_json(client.lint()))
            else:
                print(f"unknown subcommand: {args.subcmd}")
                return 2
        except CLIENT_ERRORS as exc:
            print(f"hermes aidememo {args.subcmd} failed: {exc}")
            return 1
        return 0

    return _handler


def register(ctx: Any, client: AideMemoClient) -> None:
    ctx.register_cli_command(
        name="aidememo",
        help="AideMemo subcommands (query / search / recent / add / stats / lint)",
        setup_fn=_setup,
        handler_fn=_handler_factory(client),
        description="Talk to the AideMemo knowledge graph from the hermes CLI.",
    )
