# Session JSON manifest reference

`deb-toolkit session apply <session-dir> <plan.json>` runs a sequence of
`session` operations described by a JSON file, in a single process.

The manifest format is the **declarative twin** of the CLI verb chain.
Every CLI verb has an equivalent manifest step; both modes dispatch into
the same underlying `Session` methods, so there is no behavioural drift
between them.

This doc is the schema reference and design rationale. For end-to-end
usage examples, see `examples/manifests/`.

## Why two modes?

The CLI verbs (`session rename-package`, `session insert`, …) are great
for interactive use, one-off scripts, and the migration of pre-existing
bash wrappers. Manifests are better whenever the *transformation itself*
is the artifact you want to check in, code-review, and run reproducibly:

| Situation                          | Mode               |
| ---------------------------------- | ------------------ |
| Interactive poking, one-off ops    | CLI verbs          |
| Wrapper bash scripts on dev boxes  | CLI verbs          |
| CI-driven release variants         | JSON manifest      |
| Anything reviewed in a PR          | JSON manifest      |
| Reproducible release engineering   | JSON manifest      |

Anything achievable in one is achievable in the other.

## Quick start

```bash
# Open a session.
deb-toolkit session open example-app_1.0.0_amd64.deb /tmp/session

# Apply a checked-in manifest.
deb-toolkit session apply /tmp/session ./variant-bundle/plan.json

# Save the result.
deb-toolkit session save /tmp/session example-app-variant_2.0.0_amd64.deb --verify
```

## Manifest shape

```json
{
  "description": "Optional human-readable summary, logged at INFO.",
  "steps": [
    { "op": "<verb>", "<field>": "..." }
  ]
}
```

* `description` (optional) — logged when the plan runs. Useful for
  tagging CI logs with the intent of the change.
* `steps` (required) — array of operations, applied in order. An empty
  array is valid (no-op).

**Unknown fields anywhere in the document fail parsing.** This is
intentional: a typo in a field name should not silently change
behaviour. Catch the typo at parse time rather than after CI is green.

## Operations

Each step is an object whose `op` field selects the operation. The
remaining fields are the operation's parameters. Operations correspond
1:1 to CLI verbs.

### `rename-package` — set the `Package:` control field

```json
{ "op": "rename-package", "new_name": "example-app-variant" }
```

| Field      | Type   | Required | Description                          |
| ---------- | ------ | -------- | ------------------------------------ |
| `new_name` | string | yes      | New value for the `Package:` field.  |

CLI equivalent: `session rename-package <session-dir> <new_name>`

---

### `replace-suite` — set the `Suite:` control field

```json
{ "op": "replace-suite", "new_suite": "experimental" }
```

| Field       | Type   | Required | Description                           |
| ----------- | ------ | -------- | ------------------------------------- |
| `new_suite` | string | yes      | New value. Field appended if missing. |

CLI equivalent: `session replace-suite <session-dir> <new_suite>`

---

### `reversion` — set `Version:` and rewrite versioned dep constraints

```json
{ "op": "reversion", "new_version": "2.0.0" }
```

| Field         | Type   | Required | Description                                                                                                                                |
| ------------- | ------ | -------- | ------------------------------------------------------------------------------------------------------------------------------------------ |
| `new_version` | string | yes      | New value for the `Version:` field.                                                                                                        |
| `update_deps` | bool   | no       | Deprecated and ignored — the dependency rewrite below is now unconditional. Retained so older manifests keep parsing.                       |

Reversion rewrites every dependency constraint that pinned the old version —
for **any** relation operator (`=`, `<<`, `<=`, `>=`, `>>`) — across `Depends`,
`Pre-Depends`, `Recommends`, `Suggests`, `Enhances`, `Breaks`, `Conflicts`,
`Replaces`, `Provides`. All operators are rewritten, not just `=`, because a
reversion can *lower* the version: a left-alone `(>= old)` would then be
unsatisfiable and the package uninstallable. This matches the unconditional
dependency rewrite the bash `deb-session-reversion.sh` performs on every call.
The rewrite is constraint-scoped (only inside `(...)`), so a version-like
substring in a package name is never mangled.

CLI equivalent: `session reversion <session-dir> <new_version>`

---

### `insert` — copy local files into the package

```json
{ "op": "insert",
  "dest": "/var/lib/example/data.bin",
  "sources": ["./data.bin"] }
```

```json
{ "op": "insert",
  "dest": "/var/lib/example",
  "sources": ["./payloads/a.bin", "./payloads/b.bin"],
  "directory": true }
```

| Field       | Type           | Required | Description                                                                                |
| ----------- | -------------- | -------- | ------------------------------------------------------------------------------------------ |
| `dest`      | string         | yes      | Destination *package* path (begins with `/`).                                              |
| `sources`   | array[string]  | yes      | One or more *local* file paths. Relative paths are resolved against the manifest directory. |
| `directory` | bool           | no       | If `true`, `dest` is treated as a directory and each source keeps its basename. Default `false`. |

When `directory == false` exactly one source is allowed and `dest` is
the exact destination filename. When `directory == true`, multiple
sources are placed inside `dest/` keeping their basenames.

CLI equivalent: `session insert [-d] <session-dir> <dest> <source>...`

---

### `remove` — delete package paths matching a glob

```json
{ "op": "remove", "pattern": "/var/lib/example/old_*.bin" }
```

| Field     | Type   | Required | Description                          |
| --------- | ------ | -------- | ------------------------------------ |
| `pattern` | string | yes      | Glob pattern in package-path space.  |

Fails if zero files match. Use a more permissive pattern (or guard with
`read-field`) if you need conditional removes.

CLI equivalent: `session remove <session-dir> <pattern>`

---

### `move` — rename a path within the package

```json
{ "op": "move",
  "source": "/usr/share/keep-me.txt",
  "destination": "/usr/share/moved.txt" }
```

| Field         | Type   | Required | Description           |
| ------------- | ------ | -------- | --------------------- |
| `source`      | string | yes      | Source package path.  |
| `destination` | string | yes      | Destination path.     |

CLI equivalent: `session move <session-dir> <source> <destination>`

---

### `replace` — overwrite package paths matching a glob with a local file

```json
{ "op": "replace",
  "pattern": "/etc/example/config.json",
  "replacement": "./config.json" }
```

| Field         | Type   | Required | Description                                                                              |
| ------------- | ------ | -------- | ---------------------------------------------------------------------------------------- |
| `pattern`     | string | yes      | Glob pattern in package-path space.                                                      |
| `replacement` | string | yes      | Local file to copy into every matching path. Relative paths resolved against manifest dir. |

CLI equivalent: `session replace <session-dir> <pattern> <replacement>`

---

### `read-field` — read a control-file field, optionally assert its value

```json
{ "op": "read-field", "field": "Package", "expected": "example-app-variant" }
```

| Field      | Type   | Required | Description                                                                                                |
| ---------- | ------ | -------- | ---------------------------------------------------------------------------------------------------------- |
| `field`    | string | yes      | Control-file field name (`Package`, `Version`, `Suite`, …).                                                |
| `expected` | string | no       | When present, the step fails with a useful error unless the actual value matches exactly. Default: no assertion. |

`read-field` is primarily an *assertion primitive* for CI plans — drop
one after a `reversion` step to pin the new version and catch regressions
in the verb itself before they ship.

CLI equivalent: `session read-field <session-dir> <field>`

---

## Path resolution rules

There are two distinct kinds of path in a manifest. They are resolved
differently:

### Package paths

`dest`, `pattern`, and `source`/`destination` for `move` are paths
**inside the .deb**. They begin with `/` and are mapped under
`<session>/data/` by the session engine. They are never reinterpreted
based on where the manifest file lives, the cwd, or anything else.

### Local source paths

`sources` in `insert` and `replacement` in `replace` are paths **on the
local filesystem**. They follow these rules:

* **Absolute paths** (`/tmp/x.tar.gz`, `~/...` after shell expansion) —
  used as given.
* **Relative paths** (`./x.tar.gz`, `data/x.tar.gz`, `../shared/x`) —
  resolved against the **manifest file's parent directory**, *not* the
  process cwd.

The reason: a folder containing a `plan.json` plus its referenced data
files becomes a self-contained, portable, hashable transformation
bundle. Move the directory anywhere, the manifest still works.

```
variant-bundle/
├── plan.json     # references the two files below by relative path
├── data.bin
└── config.json
```

```bash
# Works regardless of cwd.
deb-toolkit session apply /tmp/session ./variant-bundle/plan.json
deb-toolkit session apply /tmp/session /elsewhere/variant-bundle/plan.json
(cd /tmp && deb-toolkit session apply /tmp/session ~/variant-bundle/plan.json)
```

## Dry runs

Pass `--dry-run` to `session apply` to validate the manifest against
an opened session **without committing any change**:

```bash
deb-toolkit session apply /tmp/session ./plan.json --dry-run
```

What `--dry-run` checks:

- **Parse** — schema is well-formed (unknown ops, missing required
  fields, wrong types all fail before the first step runs).
- **Local files exist** — every `sources` entry in `insert` and every
  `replacement` in `replace` is resolved against the manifest
  directory and verified to point at an existing file.
- **Step shape** — `insert` with `directory: false` and multiple
  sources is rejected (since the corresponding real run would fail).

What `--dry-run` does **not** check:

- **Glob match counts.** `remove` and `replace` patterns are evaluated
  against the live data tree, so we can't tell at dry-run time
  whether a pattern will match zero files (which would fail) without
  scanning the session.
- **`read-field` assertions.** Their value depends on the control
  file's current state, which earlier (unapplied) steps would have
  changed.
- **Permission / disk-space issues** at write time.

So `--dry-run` is a CI gate against the manifest itself — it catches
the typos and broken bundle paths early, but doesn't promise a clean
real run.

The session directory is left bit-for-bit unchanged.

## Failure semantics

Steps run sequentially. On the first failure, `apply` returns the error
and stops; the remaining steps are skipped.

**There is no automatic rollback** of the session directory. By design:

* Session mutations are cheap — they live entirely in a temp directory.
* The original `.deb` is never touched until `session save`, so a
  half-applied session cannot corrupt anything important.
* Persistent step-state tracking would add significant complexity for a
  rarely-exercised "resume" path.

Recovery is therefore:

1. Read the error (which names the failing step and includes any
   context the underlying operation produced).
2. Either fix the manifest and re-run `session open` + `session apply`
   from a fresh session, or finish the remaining steps manually with
   CLI verbs.

If you want guaranteed all-or-nothing, snapshot the session directory
before running `apply`:

```bash
cp -r /tmp/session /tmp/session.backup
deb-toolkit session apply /tmp/session ./plan.json \
  || (rm -rf /tmp/session && mv /tmp/session.backup /tmp/session)
```

## Idempotency

Individual operations are **not idempotent**:

* `remove` of the same pattern twice fails the second time (zero matches).
* `reversion` from `4.0.0` to `4.0.0` is a no-op, but reversioning back
  from `4.0.0` to `3.0.0` is a real change.
* `insert` of the same dest twice will simply overwrite — but if you
  have removed the source between runs, it will fail with a different
  error.

**Manifests are designed to be run once against a fresh session.** To
re-run, delete the session directory and `session open` again. The
deterministic-tar guarantee on `session save` means re-running an
unchanged manifest from a fresh session produces a byte-identical
`.deb`.

## Schema validation

The parser is strict:

* Unknown ops (`"op": "destroy-everything"`) — rejected.
* Unknown fields on a known op — rejected.
* Missing required fields — rejected.
* Wrong types — rejected.

All schema errors include the offending field name and the input
location, courtesy of `serde_json`'s error reporting.

## Worked example

See [`examples/manifests/variant-bundle/`](../examples/manifests/variant-bundle/)
for a complete, runnable bundle that rebrands a stable package as an
experimental variant.
