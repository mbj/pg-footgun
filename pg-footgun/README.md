# pg-footgun

CLI that builds patched PostgreSQL OCI images making silent PostgreSQL footguns
visible — turning a silently-handled hazard into an explicit `WARNING`/`ERROR`.

See `DESIGN.md` for the full design.

## Commands

Build commands take a platform-qualified target — `upstream-18.4` / `debian-18.4`
(or `all`). `upstream-*` builds from a git checkout; `debian-*` rebuilds the
pinned PGDG source package. `master` and betas are upstream-only.

- `setup-bare` — create/update the shared upstream bare clone.
- `checkout <target|all>` — check out pristine upstream source.
- `unpack <target|all>` — unpack a pinned PGDG source package.
- `apply <target|all>` — apply the footgun patch(es).
- `build <target|all>` — apply patches and build.
- `test <target|all>` — build and run the regression suite.
- `list-patches` — list the committed patches and their targets.
