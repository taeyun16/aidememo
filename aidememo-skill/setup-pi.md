---
kind: doc
title: pi coding agent setup guide
---

# Using AideMemo with pi coding agent

pi uses Agent Skills and its built-in `bash` tool instead of MCP. AideMemo is
installed into pi's native skill directory, so the full instructions do not
need to be injected into every turn.

Korean: [`setup-pi.ko.md`](setup-pi.ko.md)

## Install

```bash
cd ~/dev/aidememo
cargo build -p aidememo-cli --release
export PATH="$PWD/target/release:$PATH"
aidememo skill install --target pi
```

The default destination is `~/.pi/agent/skills/aidememo/`. Set the environment
variable when using a separate pi profile:

```bash
export PI_CODING_AGENT_DIR="$HOME/.pi/work-profile"
aidememo skill install --target pi
```

## Verify

Start a new pi session and invoke the skill directly:

```text
/skill:aidememo
```

You can also ask naturally:

```text
Find our decisions about Redis timeouts in the project memory.
Record this decision in AideMemo as a decision fact.
```

pi follows the skill instructions and runs local CLI commands such as:

```bash
aidememo --json query "Redis timeout" --bm25-only
aidememo fact add "Redis timeout is 30 seconds" \
  --type decision --entities Redis
```

There is no MCP registration step for pi. If the installation output suggests
`mcp-install --target pi`, you are running an older AideMemo binary and should
rebuild or update it.
