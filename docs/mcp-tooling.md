# MCP Tooling Reference

MCP servers are configured externally and may not all be available in every environment. If present, use as follows.

## Available Servers

| Tool | Use for | Setup |
|---|---|---|
| **Serena** | Code navigation and editing: `find_symbol`, `get_symbols_overview`, `replace_symbol_body`, `find_referencing_symbols` | Separate (project-specific `--project` path) |
| **Ripgrep** | Fast file/content search. Use instead of CLI `grep`/`rg`/`find` (user preference) | `.mcp.json` |
| **Context7** | External library docs lookup (subxt, axum, sqlx, alloy, tokio). Always check before assuming APIs | Separate (requires personal API key) |
| **Exa** | Online search, code samples, best practices (when Context7 lacks coverage) | `.mcp.json` |
| **mcp-server-git** | Git read ops (status, diff, log). Use Bash for complex git | `.mcp.json` |
| **Playwright** | Browser automation for testing local dev server | `.mcp.json` |
| **Sequential Thinking** | Step-by-step reasoning for complex problems | `.mcp.json` |

### Servers requiring separate installation

**Serena** — configured per-project in `~/.claude.json` (not `.mcp.json`) because the `--project` arg must point to the current repo root:
```json
"serena": {
  "type": "stdio",
  "command": "uvx",
  "args": ["--from", "git+https://github.com/oraios/serena", "serena",
           "start-mcp-server", "--context", "claude-code",
           "--project", "/path/to/Kalatori"]
}
```

**Context7** — requires a personal API key from [Upstash](https://context7.com). Configure per-project or globally:
```json
"context7": {
  "type": "stdio",
  "command": "npx",
  "args": ["-y", "@upstash/context7-mcp", "--api-key", "YOUR_KEY"]
}
```

## Usage Patterns

### Code Navigation (Serena)
- Use `get_symbols_overview` for file-level overview before reading entire files
- Use `find_symbol` with `include_body=True` only for symbols you need to understand or edit
- Use `find_referencing_symbols` to understand call sites before refactoring
- Prefer Serena's symbolic tools over reading entire files — saves context tokens

### Library Documentation (Context7)
- `resolve-library-id` → `query-docs`
- Key libraries to look up: `subxt` (Substrate client), `axum` (HTTP), `sqlx` (database), `alloy` (EVM), `tokio` (async runtime)
- Never assume library APIs from memory — always verify

### Code Search (Ripgrep)
- Use ripgrep MCP for all code search (preferred over `grep` via Bash)
- Supports regex, file type filtering, context lines

### Online Research (Exa)
- `web_search_exa` for general questions about Rust, Substrate, Polkadot, Polygon
- `get_code_context_exa` for finding code examples and patterns
- Use when Context7 doesn't have the answer
