# WikiGraph (wg)

Structured wiki indexing with hybrid search.

## Quick Start
- `wg init <path>` — initialize a wiki store
- `wg ingest <path>` — ingest markdown files
- `wg search <query>` — search facts
- `wg model download minishlab/potion-multilingual-128M` — download embedding model

## Architecture
- wg-core: core library (redb + BM25 + semantic)
- wg-cli: CLI tool
- wg-napi: Node.js bindings
- wg-python: Python bindings
- wg-nif: Elixir bindings
- wg-ffi: C-ABI bindings

## Features
- BM25 keyword search
- Semantic vector search (Model2Vec)
- Hybrid search (RRF fusion)
- MCP server mode
- Fact extraction from markdown
- Lint & graph health

## License: MIT
