# ponte

> Bridges LangChain OpenWiki into the pleme-io fleet: reads the target repo's typescape IR + zoekt hits as context, invokes OpenWiki unmodified as a subprocess, routes the result through the fleet's doc/compliance layers.

OpenWiki has no plugin API (`tools: []` is hardcoded in its own agent
loop). ponte never forks it — it shapes the environment OpenWiki's
own shell-exploring agent reads (a fresh `docs/typescape-summary.md`
every run) and reconciles what OpenWiki writes back into the repo's
`CLAUDE.md` (a pointer, never a duplicated copy).

## Pipeline

Three `shigoto` Jobs run as one Dag per invocation:

```
ContextAssembleJob -> InvokeOpenWikiJob -> RouteJob
```

- **ContextAssembleJob** — reads `.typescape.yaml` (if present) + a
  best-effort zoekt lookup, writes `docs/typescape-summary.md`, and
  persists a content hash so unrelated reruns are true no-ops.
- **InvokeOpenWikiJob** — subprocess-execs the `openwiki` binary,
  auto-detecting `--init` vs `--update` by whether `openwiki/` already
  exists. Skipped entirely when nothing changed upstream.
- **RouteJob** — ensures `CLAUDE.md` points at `openwiki/quickstart.md`,
  idempotently.

Every transition is recorded to `~/.ponte/audit/<repo-slug>.jsonl`.

## Usage

```bash
ponte --repo /path/to/target-repo --openwiki-bin /path/to/openwiki
```

## Building

```bash
nix run .#ponte -- --help
```

## Known gap: OpenWiki packaging

Phase 0 added `mkNpmTool` to `substrate` (wraps `buildNpmPackage`) to
package OpenWiki as a Nix derivation — but OpenWiki is actually a
**pnpm workspace** (`pnpm-lock.yaml` + `pnpm-workspace.yaml`, no
`package-lock.json`), discovered only once a real build was attempted.
`mkNpmTool` is still correct for npm-lockfile tools; it just isn't the
right builder for OpenWiki specifically. Until a pnpm-aware substrate
builder lands, point `--openwiki-bin` at an OpenWiki you've installed
yourself (`pnpm install -g openwiki` or equivalent).

## License

MIT.
