//! Apply a JSON manifest of session operations in a single process.
//!
//! # Why a manifest mode exists
//!
//! The CLI verbs (`session rename-package`, `session insert`, …) are great
//! for interactive use and ad-hoc scripting, but every invocation is a fresh
//! process that re-loads `metadata.env` and re-parses arguments. When the
//! "transformation" itself is the artifact you want to check in, code-review,
//! and run reproducibly in CI, a declarative manifest is a better fit:
//!
//!   * **One process, one outcome.** The full sequence either completes
//!     or aborts at the failing step. The original `.deb` is never touched
//!     until `session save`, so a partially-applied session can be discarded
//!     by deleting the session directory.
//!   * **Self-documenting.** The manifest *is* the change list. Reviewers
//!     see exactly which files move, which fields flip, and in what order
//!     — without grepping through a wrapper bash script.
//!   * **Path-relative.** Source paths in the manifest are resolved against
//!     the manifest's own directory, so a folder containing
//!     `plan.json` + `data.bin` + `config.json`
//!     is a fully portable transformation bundle.
//!
//! # When to reach for which mode
//!
//! | Situation                          | Mode               |
//! | ---------------------------------- | ------------------ |
//! | Interactive poking, one-off ops    | CLI verbs          |
//! | Wrapper bash scripts on dev boxes  | CLI verbs          |
//! | CI-driven release variants         | JSON manifest      |
//! | Anything reviewed in a PR          | JSON manifest      |
//! | Reproducible release engineering   | JSON manifest      |
//!
//! Both modes dispatch into the same [`Session`] methods, so there is no
//! behavioural drift between them. Anything achievable in one is achievable
//! in the other.
//!
//! # Manifest schema (informal)
//!
//! ```json
//! {
//!   "description": "Optional human-readable summary, logged at INFO.",
//!   "steps": [
//!     { "op": "remove",          "pattern": "/var/lib/example/old.bin" },
//!     { "op": "insert",          "dest": "/var/lib/example/data.bin",
//!                                "sources": ["./data.bin"] },
//!     { "op": "rename-package",  "new_name": "example-app-variant" },
//!     { "op": "reversion",       "new_version": "2.0.0", "update_deps": true },
//!     { "op": "replace-suite",   "new_suite": "experimental" },
//!     { "op": "read-field",      "field": "Package",
//!                                "expected": "example-app-variant" }
//!   ]
//! }
//! ```
//!
//! See [`Step`] for the full list of ops, and `docs/session-manifest.md`
//! for the long-form reference + worked examples.
//!
//! # Path resolution rules
//!
//! There are two kinds of path in a manifest:
//!
//! * **Package paths** (`dest`, `pattern`, `source`/`destination` for
//!   `move`). These describe locations *inside the .deb* — they begin
//!   with `/` and are resolved against `<session>/data/` by
//!   [`Session::resolve_package_path`]. They are never reinterpreted
//!   based on where the manifest lives.
//!
//! * **Local source paths** (`sources` in `insert`, `replacement` in
//!   `replace`). These describe files *on the dev box / runner* that
//!   should be copied into the package. If absolute they are used as
//!   given; if relative they are resolved against the **manifest file's
//!   parent directory**, not the process's cwd. This makes a manifest
//!   plus its companion data files into a portable, hashable bundle.
//!
//! # Failure semantics
//!
//! Steps run sequentially. On the first failure, [`apply`] returns the
//! error and stops; remaining steps are skipped. There is **no automatic
//! rollback** of the session directory — by design, because session
//! mutations are already cheap (they live entirely in a temp dir) and
//! the original `.deb` is untouched.
//!
//! Recovery is therefore: read the error, either fix the manifest and
//! re-run `session open` + `session apply` from a fresh session, or
//! finish the remaining steps manually with CLI verbs. Sessions are
//! intentionally not re-entrant on a half-applied manifest because that
//! would require persistent step-state tracking, which is more
//! complexity than the use case warrants.
//!
//! # Idempotency
//!
//! Individual ops are *not* idempotent: removing the same file twice
//! fails (the second remove finds zero matches), reversioning from
//! `4.0.0` to `4.0.0` is a no-op but reversioning back from `4.0.0` to
//! `3.0.0` is a real change. Manifests are therefore meant to be run
//! *once* against a fresh session, not repeatedly against an
//! incrementally-mutated one. To re-run, blow away the session
//! directory and `session open` again.

use anyhow::{anyhow, Context, Result};
use serde::Deserialize;
use std::path::{Path, PathBuf};

use super::Session;

/// Top-level manifest document.
///
/// Deserialized from a JSON file by [`Plan::load`].
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Plan {
    /// Optional human-readable description, logged at INFO when the plan
    /// runs. Useful for tagging CI logs with the intent of the change.
    #[serde(default)]
    pub description: Option<String>,
    /// Steps applied in order. Empty step lists are accepted (no-op).
    pub steps: Vec<Step>,
}

/// A single manifest step. Each variant corresponds 1:1 to a `session`
/// CLI verb; the serde tag is `op`, in kebab-case.
///
/// `#[serde(deny_unknown_fields)]` is set on each variant so typos in
/// field names produce a parse error rather than silently being ignored.
#[derive(Debug, Deserialize)]
#[serde(tag = "op", rename_all = "kebab-case", deny_unknown_fields)]
pub enum Step {
    /// `session rename-package <new_name>` — set the `Package:` field.
    RenamePackage {
        /// New value for the `Package:` control field.
        new_name: String,
    },

    /// `session replace-suite <new_suite>` — set the `Suite:` field.
    ReplaceSuite {
        /// New value for the `Suite:` control field. Appended if missing.
        new_suite: String,
    },

    /// `session reversion <new_version>` — set the `Version:` field and rewrite
    /// every versioned dependency constraint that pinned the old version, for
    /// any operator (`=`, `<<`, `<=`, `>=`, `>>`).
    Reversion {
        /// New value for the `Version:` control field.
        new_version: String,
        /// Deprecated and ignored: the dependency rewrite is now unconditional.
        /// Retained so manifests that still carry `update_deps` keep parsing.
        #[serde(default)]
        update_deps: bool,
    },

    /// `session insert [-d] <dest> <source>...` — copy one or more files
    /// from the local filesystem into the package.
    ///
    /// When `directory == false` and `sources.len() == 1`, the single
    /// source file is placed at exactly the package path `dest`.
    ///
    /// When `directory == true`, `dest` is treated as a directory and
    /// each source file keeps its basename inside that directory.
    Insert {
        /// Destination *package* path (inside `/`), e.g. `/var/lib/example/data.bin`.
        dest: String,
        /// One or more *local* source files. Relative paths are resolved
        /// against the manifest file's parent directory.
        sources: Vec<String>,
        /// Treat `dest` as a directory; each source keeps its basename.
        #[serde(default)]
        directory: bool,
    },

    /// `session remove <pattern>` — delete every package path matching
    /// the glob. Fails if zero files match.
    Remove {
        /// Glob pattern in package-path space (anchored at `/`).
        pattern: String,
    },

    /// `session move <source> <destination>` — rename a package path.
    Move {
        /// Source *package* path.
        source: String,
        /// Destination *package* path.
        destination: String,
    },

    /// `session replace <pattern> <replacement>` — overwrite every
    /// package path matching the glob with the content of a single
    /// local file. Useful for swapping config files in bulk.
    Replace {
        /// Glob pattern in package-path space.
        pattern: String,
        /// Local source file. Relative paths are resolved against the
        /// manifest file's parent directory.
        replacement: String,
    },

    /// `session read-field <field>` — read a control-file field and log
    /// its value. If `expected` is set, the step fails unless the
    /// actual value matches exactly. Designed for CI assertions: pin
    /// `Version` to confirm an earlier `reversion` step really took.
    ReadField {
        /// Control-file field name (`Package`, `Version`, …).
        field: String,
        /// Optional exact-match expectation. When set and unequal,
        /// `apply` aborts with a useful error.
        #[serde(default)]
        expected: Option<String>,
    },
}

impl Plan {
    /// Read and parse a manifest from `path`.
    ///
    /// Returns a descriptive error if the file is missing, unreadable,
    /// or fails JSON-schema validation (unknown fields, missing required
    /// fields, wrong types, …).
    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("Reading manifest {}", path.display()))?;
        let plan: Plan = serde_json::from_str(&text)
            .with_context(|| format!("Parsing manifest {}", path.display()))?;
        Ok(plan)
    }
}

/// Run every step of `plan` against `session`.
///
/// `manifest_dir` is the directory used to resolve relative local-source
/// paths inside `Insert` and `Replace` steps. Pass the parent directory
/// of the manifest file you loaded; this is what makes a manifest+data
/// bundle portable. For programmatically-constructed plans, pass the
/// directory the source files live in (often the process cwd).
///
/// When `dry_run` is true, no session mutations are performed: each
/// step's intent is logged, local source files referenced by `insert`
/// and `replace` are verified to exist, and parse-level errors still
/// surface — but the control file and data tree under `<session>/` are
/// left untouched. Useful for CI gates that want to validate a manifest
/// against an opened session without committing to the change.
///
/// Errors abort the apply: see [module-level docs](self) for the
/// failure-semantics rationale.
pub fn apply(session: &Session, plan: &Plan, manifest_dir: &Path, dry_run: bool) -> Result<()> {
    if let Some(desc) = &plan.description {
        log::info!("Plan: {}", desc);
    }
    if dry_run {
        log::info!(
            "=== DRY RUN: would apply {} step(s), no changes will be made ===",
            plan.steps.len()
        );
    } else {
        log::info!("=== Applying {} step(s) ===", plan.steps.len());
    }

    for (idx, step) in plan.steps.iter().enumerate() {
        let step_num = idx + 1;
        log::info!("[{}/{}] {}", step_num, plan.steps.len(), step.summary());
        if dry_run {
            check_step(step, manifest_dir).with_context(|| {
                format!(
                    "Step {} of {} would fail ({})",
                    step_num,
                    plan.steps.len(),
                    step.summary()
                )
            })?;
        } else {
            apply_step(session, step, manifest_dir).with_context(|| {
                format!(
                    "Step {} of {} failed ({})",
                    step_num,
                    plan.steps.len(),
                    step.summary()
                )
            })?;
        }
    }

    if dry_run {
        log::info!(
            "✓ Dry run complete: {} step(s) validated, no changes applied",
            plan.steps.len()
        );
    } else {
        log::info!("✓ All {} step(s) applied", plan.steps.len());
    }
    Ok(())
}

/// Dry-run validation for a single step. Only checks what can be
/// verified without mutating the session: that local source files
/// referenced by `insert` / `replace` exist on disk, and that step
/// fields are individually well-formed. Does **not** verify that
/// `remove` / `move` patterns will match anything, since glob
/// matching requires looking at the live data tree.
fn check_step(step: &Step, manifest_dir: &Path) -> Result<()> {
    match step {
        Step::Insert {
            sources, directory, ..
        } => {
            if sources.is_empty() {
                return Err(anyhow!("insert: `sources` is empty"));
            }
            if !*directory && sources.len() > 1 {
                return Err(anyhow!(
                    "insert: {} sources but `directory` is false — must be true to use multiple sources",
                    sources.len()
                ));
            }
            for s in sources {
                let p = resolve_local(manifest_dir, s);
                if !p.exists() {
                    return Err(anyhow!(
                        "insert: source file does not exist: {}",
                        p.display()
                    ));
                }
            }
            Ok(())
        }
        Step::Replace { replacement, .. } => {
            let p = resolve_local(manifest_dir, replacement);
            if !p.exists() {
                return Err(anyhow!(
                    "replace: replacement file does not exist: {}",
                    p.display()
                ));
            }
            Ok(())
        }
        // The remaining ops are pure metadata mutations or glob-driven
        // operations whose outcome depends on session state. They have
        // nothing to validate at dry-run time beyond what the schema
        // already enforced at parse time.
        Step::RenamePackage { .. }
        | Step::ReplaceSuite { .. }
        | Step::Reversion { .. }
        | Step::Remove { .. }
        | Step::Move { .. }
        | Step::ReadField { .. } => Ok(()),
    }
}

impl Step {
    /// A short one-line label for the step, used in log lines and error
    /// context so the user can pinpoint which step failed without
    /// debug-printing the full enum payload.
    fn summary(&self) -> String {
        match self {
            Step::RenamePackage { new_name } => format!("rename-package → {}", new_name),
            Step::ReplaceSuite { new_suite } => format!("replace-suite → {}", new_suite),
            Step::Reversion {
                new_version,
                update_deps: _,
            } => format!("reversion → {}", new_version),
            Step::Insert {
                dest,
                sources,
                directory,
            } => {
                if *directory {
                    format!("insert -d {} ← {} file(s)", dest, sources.len())
                } else {
                    format!("insert {} ← {:?}", dest, sources)
                }
            }
            Step::Remove { pattern } => format!("remove {}", pattern),
            Step::Move {
                source,
                destination,
            } => format!("move {} → {}", source, destination),
            Step::Replace {
                pattern,
                replacement,
            } => format!("replace {} ← {}", pattern, replacement),
            Step::ReadField { field, expected } => match expected {
                Some(e) => format!("read-field {} == {:?}", field, e),
                None => format!("read-field {}", field),
            },
        }
    }
}

fn apply_step(session: &Session, step: &Step, manifest_dir: &Path) -> Result<()> {
    match step {
        Step::RenamePackage { new_name } => session.rename_package(new_name),

        Step::ReplaceSuite { new_suite } => session.replace_suite(new_suite),

        Step::Reversion {
            new_version,
            update_deps: _,
        } => session.reversion(new_version),

        Step::Insert {
            dest,
            sources,
            directory,
        } => {
            if sources.is_empty() {
                return Err(anyhow!("insert: `sources` is empty"));
            }
            let resolved: Vec<PathBuf> = sources
                .iter()
                .map(|s| resolve_local(manifest_dir, s))
                .collect();
            session.insert(dest, &resolved, *directory)
        }

        Step::Remove { pattern } => {
            let n = session.remove(pattern)?;
            log::info!("    removed {} file(s)", n);
            Ok(())
        }

        Step::Move {
            source,
            destination,
        } => session.move_path(source, destination),

        Step::Replace {
            pattern,
            replacement,
        } => {
            let local = resolve_local(manifest_dir, replacement);
            let n = session.replace(pattern, &local)?;
            log::info!("    replaced {} file(s)", n);
            Ok(())
        }

        Step::ReadField { field, expected } => {
            let value = session.read_field(field)?;
            log::info!("    {}: {}", field, value);
            if let Some(exp) = expected {
                if &value != exp {
                    return Err(anyhow!(
                        "read-field assertion failed for `{}`:\n  expected: {:?}\n    actual: {:?}",
                        field,
                        exp,
                        value
                    ));
                }
            }
            Ok(())
        }
    }
}

/// Resolve a manifest-relative local path against the manifest's parent
/// directory. Absolute paths are returned unchanged. See module-level
/// "Path resolution rules" for the rationale.
fn resolve_local(manifest_dir: &Path, p: &str) -> PathBuf {
    let path = Path::new(p);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        manifest_dir.join(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_full_manifest() {
        let json = r#"{
            "description": "test plan",
            "steps": [
                { "op": "rename-package", "new_name": "foo" },
                { "op": "replace-suite",  "new_suite": "stable" },
                { "op": "reversion", "new_version": "2.0", "update_deps": true },
                { "op": "insert", "dest": "/x", "sources": ["a", "b"], "directory": true },
                { "op": "remove", "pattern": "/y/*" },
                { "op": "move", "source": "/a", "destination": "/b" },
                { "op": "replace", "pattern": "/c/*", "replacement": "./new" },
                { "op": "read-field", "field": "Package", "expected": "foo" }
            ]
        }"#;
        let plan: Plan = serde_json::from_str(json).unwrap();
        assert_eq!(plan.description.as_deref(), Some("test plan"));
        assert_eq!(plan.steps.len(), 8);
    }

    #[test]
    fn reversion_parses_without_update_deps() {
        // The field is deprecated and ignored, but a manifest may omit it
        // (serde default) or still carry it — both must parse.
        let json = r#"{ "steps": [
            { "op": "reversion", "new_version": "2.0" }
        ] }"#;
        let plan: Plan = serde_json::from_str(json).unwrap();
        match &plan.steps[0] {
            Step::Reversion { new_version, .. } => assert_eq!(new_version, "2.0"),
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn unknown_op_is_rejected() {
        let json = r#"{ "steps": [{ "op": "destroy-everything" }] }"#;
        let err = serde_json::from_str::<Plan>(json).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("destroy-everything") || msg.contains("unknown variant"));
    }

    #[test]
    fn unknown_field_is_rejected() {
        let json = r#"{ "steps": [{ "op": "rename-package", "new_name": "x", "typo": 1 }] }"#;
        let err = serde_json::from_str::<Plan>(json).unwrap_err();
        assert!(err.to_string().contains("typo") || err.to_string().contains("unknown field"));
    }

    #[test]
    fn empty_steps_is_valid_noop() {
        let plan: Plan = serde_json::from_str(r#"{ "steps": [] }"#).unwrap();
        assert!(plan.steps.is_empty());
    }

    #[test]
    fn resolve_local_absolute_unchanged() {
        let r = resolve_local(Path::new("/manifests"), "/abs/path");
        assert_eq!(r, PathBuf::from("/abs/path"));
    }

    #[test]
    fn resolve_local_relative_joins_manifest_dir() {
        let r = resolve_local(Path::new("/manifests"), "data/foo.tar.gz");
        assert_eq!(r, PathBuf::from("/manifests/data/foo.tar.gz"));
    }

    // --- check_step (dry-run validation) ---------------------------------

    #[test]
    fn check_step_insert_rejects_missing_source() {
        let tmp = tempfile::tempdir().unwrap();
        let step = Step::Insert {
            dest: "/x".into(),
            sources: vec!["./missing.bin".into()],
            directory: false,
        };
        let err = check_step(&step, tmp.path()).unwrap_err();
        assert!(err.to_string().contains("does not exist"), "{}", err);
    }

    #[test]
    fn check_step_insert_accepts_existing_source() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("there.bin"), b"x").unwrap();
        let step = Step::Insert {
            dest: "/x".into(),
            sources: vec!["./there.bin".into()],
            directory: false,
        };
        check_step(&step, tmp.path()).unwrap();
    }

    #[test]
    fn check_step_insert_rejects_multi_source_without_directory_flag() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("a"), b"a").unwrap();
        std::fs::write(tmp.path().join("b"), b"b").unwrap();
        let step = Step::Insert {
            dest: "/x".into(),
            sources: vec!["./a".into(), "./b".into()],
            directory: false,
        };
        let err = check_step(&step, tmp.path()).unwrap_err();
        assert!(err.to_string().contains("directory"), "{}", err);
    }

    #[test]
    fn check_step_replace_rejects_missing_replacement() {
        let tmp = tempfile::tempdir().unwrap();
        let step = Step::Replace {
            pattern: "/x".into(),
            replacement: "./missing".into(),
        };
        let err = check_step(&step, tmp.path()).unwrap_err();
        assert!(err.to_string().contains("does not exist"), "{}", err);
    }

    #[test]
    fn check_step_metadata_ops_have_nothing_to_check() {
        // None of these touch the filesystem at validate-time.
        check_step(
            &Step::RenamePackage {
                new_name: "x".into(),
            },
            Path::new("/"),
        )
        .unwrap();
        check_step(
            &Step::Reversion {
                new_version: "1.0".into(),
                update_deps: false,
            },
            Path::new("/"),
        )
        .unwrap();
        check_step(
            &Step::Remove {
                pattern: "/x/*".into(),
            },
            Path::new("/"),
        )
        .unwrap();
    }
}
