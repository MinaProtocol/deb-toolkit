# deb-toolkit

[![CI](https://github.com/MinaProtocol/deb-toolkit/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/MinaProtocol/deb-toolkit/actions/workflows/ci.yml)
[![Coverage Status](https://coveralls.io/repos/github/MinaProtocol/deb-toolkit/badge.svg?branch=main)](https://coveralls.io/github/MinaProtocol/deb-toolkit?branch=main)

A small Rust CLI for building, signing, and verifying Debian packages.

## CLI

```
deb-toolkit build         --build-dir … --output-dir … --package-name … --version … \
                          --suite … --codename …   [+ optional metadata]
deb-toolkit sign          --deb … --key …
deb-toolkit verify content --deb …  [+ optional metadata]
deb-toolkit verify signature <deb> [--key <path|url>]
deb-toolkit lookup sign-key <deb>
deb-toolkit session <verb> …          (transactional .deb modification)
```

Run `deb-toolkit <subcommand> --help` for the full flag list.

## Session subsystem

`deb-toolkit session` is a transactional `.deb` editor: extract a
package into a session directory, mutate its control fields or data
files, then repack into a fresh `.deb`. The original `.deb` is never
touched until you call `session save`.

```
session open <input.deb> <session-dir>
session save <session-dir> <output.deb> [--verify]
session read-field    <session-dir> <field>
session rename-package <session-dir> <new-name>
session replace-suite  <session-dir> <new-suite>
session reversion      <session-dir> <new-version> [--update-deps]
session insert [-d]    <session-dir> <dest> <source>…
session remove         <session-dir> <pattern>
session move           <session-dir> <source> <destination>
session replace        <session-dir> <pattern> <replacement>
session apply          <session-dir> <plan.json>      (declarative mode)
```

The verbs are fine for interactive use. For CI-driven flows where the
transformation itself is a reviewed artifact, use the **JSON manifest
mode** via `session apply` — see
[`docs/session-manifest.md`](docs/session-manifest.md) for the schema
reference and [`examples/manifests/`](examples/manifests/) for worked
examples (including a complete variant-rebrand bundle).

## Build

```
cargo build --release
./target/release/deb-toolkit --help
```

## Test

```
cargo test
```

Unit tests run anywhere. The two integration tests under `tests/` need
`fakeroot`, `dpkg-deb`, `debsigs`, `debsig-verify`, and `gpg` on PATH and
are skipped (with a printed note) on systems without them.

## Tools shelled out to

`fakeroot dpkg-deb`, `debsigs`, `debsig-verify`, `gpg` (tests), `curl`.

## Control fields

`build` writes `Package`, `Version`, `Architecture`, `Maintainer`, `Section`,
`Priority`, `Homepage`, `Installed-Size`, `Source`, `Suite`, `Codename`,
`License`, and `Description` unconditionally, plus these when supplied:
`Vendor`, `Authors`, `Depends`, `Pre-Depends`, `Recommends`, `Suggests`,
`Conflicts`, `Replaces`, `Provides`.

The optional fields are omitted entirely when unset or empty, rather than
emitted with a blank value. Relationship fields are joined with `", "`.

## Known limitation

`Origin`, `Label`, and `Breaks` are not modelled and cannot be set.

## License

Apache-2.0
