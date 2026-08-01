# pg-footgun

PostgreSQL accepts a lot without comment. Some of what it accepts is a hazard, and you
only find out much later, when something is slow or wrong and nothing points at why.

pg-footgun makes those hazards say something. Each one gets a setting that turns it
into a `WARNING` you can log, or an `ERROR` that rejects the query. Every setting
defaults to `off`, so nothing changes until you ask for it.

It ships as patched PostgreSQL images, drop-in for the official ones.

## The footguns

### Joins the planner won't reorder

PostgreSQL considers alternative join orders only while the explicit `JOIN`s fit within
`join_collapse_limit`, which defaults to 8. Beyond it, the nest is planned as written:
each piece is still optimized — join methods, indexes, and the usual heuristics — but
the order you wrote is kept. If that order is a bad one, the plan is bad, and nothing
says so. Queries cross that line by accretion: an extra eager-load, another table in a
generated query, a view nested one level deeper.

`join_collapse_limit_excess` decides what happens at that point: stay silent (`off`,
the default), report it and run anyway (`warn`), or refuse the query (`error`).

```sql
SET join_collapse_limit = 2;            -- default 8, lowered to keep the example short
SET join_collapse_limit_excess = 'warn';

SELECT * FROM table_a JOIN table_b USING (id) JOIN table_c USING (id);
WARNING:  join order optimization stopped because join_collapse_limit (2) was exceeded
DETAIL:  Optimizing the join order of "table_a", "table_b", "table_c" would require considering 3 items together.
HINT:  Increase join_collapse_limit to at least 3 to let the planner choose the join order for these items.
```

`warn` suits production; `error` suits development and CI. The error is an ordinary
one — application code and PL/pgSQL can catch it.

## Using the images

The official `postgres:<version>` image with the patched server installed over it —
same base, same build, nothing else changed. Out of the box it behaves exactly like
the official image; the footgun settings are off until set.

```console
$ docker run --rm ghcr.io/mbj/postgres:18.4-pg-footgun \
    postgres -c join_collapse_limit_excess=warn
```

Two tags per supported minor: `<version>-pg-footgun-<N>` is immutable, `N` being the
footgun release counter; `<version>-pg-footgun` moves to the newest patch set for that
minor. Published minors: `18.4`, `18.3`, `17.10`, `16.14`, `15.18`, `15.17`.

Settings can be enabled per session (`SET`), per role or database (`ALTER ROLE … SET`),
or server-wide (`postgresql.conf`, or `-c` as above).

## Building it yourself

`pg-footgun`, the CLI in this repository, does everything: `setup-bare`, `checkout`,
`unpack`, `apply`, `build`, `test`, `image`, `push`, `list-patches`. Each takes a
platform-qualified target — `upstream-18.4` / `debian-18.4` — or `all`.

```console
$ cargo build --release

$ pg-footgun test upstream-18.4     # checkout, apply, build, `make check`
$ pg-footgun image debian-18.4      # unpack, apply, dpkg-buildpackage, overlay
```

`upstream-*` builds from a git checkout of `postgres/postgres` at the release tag —
the path for developing a patch and testing it against pristine upstream. `master` and
betas are upstream-only. `debian-*` rebuilds the pinned PGDG source package inside the
matching `postgres:<version>` container; that is what actually ships.

Every step is idempotent and everything lands under `target/`. Needs a Rust toolchain
(pinned in `rust-toolchain.toml`), `git`, and podman or docker for the Debian targets;
`upstream-*` additionally needs PostgreSQL's build dependencies on the host.

## Patches

```
patches/upstream/<TAG>/<footgun>.patch
```

`<TAG>` is the upstream git tag (`REL_18_4`, `master`), which is also where the
checkout lands — the ref is the identifier. The canonical patch lives at the newest
minor of each major; older minors symlink to it until one needs a real backport. The
Debian build reads the same files, so both platforms are patched from one source.

The patches are written to upstream's standards, with the intent of proposing them
there.

## License

MIT.
