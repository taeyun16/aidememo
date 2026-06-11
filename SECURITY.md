# Security Policy

## Reporting a Vulnerability

Please report security issues privately by email to Taeyun Jang at
taeyun16@pm.me. Do not open a public GitHub issue for vulnerabilities.

Include:

- the affected AideMemo version or commit;
- the vulnerable command, MCP route, SDK call, or binding surface;
- a minimal reproduction or proof of concept;
- expected impact, including whether local files, bearer tokens, or memory
  stores can be read or modified.

We will acknowledge reports within 7 days and coordinate a fix before public
disclosure when the issue is valid.

## Scope

Security-sensitive areas include:

- `aidememo mcp-serve` HTTP/SSE transport and bearer-token handling;
- `aidememo auth` token storage;
- sync, import, export, and archive paths;
- native bindings and FFI memory ownership;
- markdown ingest of untrusted repositories;
- filesystem access around store paths, project configs, and model caches.

The default local CLI and stdio MCP workflows are intended for trusted local
users on the same machine. Binding `aidememo mcp-serve` to a non-loopback
address requires authentication by design; unauthenticated non-loopback
exposure is considered a security bug.
