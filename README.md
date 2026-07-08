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

# or, using the flake-packaged OpenWiki directly:
PONTE_OPENWIKI_BIN="$(nix build .#openwiki --no-link --print-out-paths)/bin/openwiki" \
  ponte --repo /path/to/target-repo
```

## Building

```bash
nix run .#ponte -- --help
```

## OpenWiki packaging

OpenWiki is locked with `pnpm-lock.yaml`, not `package-lock.json` —
`mkNpmTool` (wraps `buildNpmPackage`) can't prefetch it. `flake.nix`
packages it via substrate's `mkPnpmTool` instead, which wraps nixpkgs'
own native `pnpm.fetchDeps` + `pnpm.configHook` (the pnpm-native
equivalent of `buildNpmPackage`'s hermetic fetch-then-offline-install
shape). Verified end-to-end: `nix build .#openwiki` produces a real,
running `openwiki --help`.

One nixpkgs/pnpm gotcha `mkPnpmTool` works around: upstream projects
that pin an exact pnpm via package.json's `packageManager` field (as
OpenWiki does) trip pnpm 10's own self-managed-version-download the
instant any `pnpm` command runs — which fails offline mid-build.
`mkPnpmTool` sets `npm_config_manage_package_manager_versions=false`
as a real env var (not a `pnpm config set`, which needs a working
`pnpm` call to land — too late for the very first one) to close that
gap before it can fire.

## License

MIT.
