//! Shared utilities for the integration tests.
//!
//! Each `tests/*.rs` integration test imports this module via
//! `mod common;`. Three things live here:
//!
//!   * [`Toolkit`] — a thin wrapper around the deb-toolkit binary with
//!     a method per CLI verb. Returns [`CmdOutput`] for assertion.
//!   * [`DebFixture`] — a fluent builder that materializes a `.deb` on
//!     disk via `dpkg-deb --build`. Replaces the inline fs::write +
//!     dpkg-deb shell-out boilerplate every test used to carry.
//!   * [`dpkg`] — `dpkg-deb --info` / `dpkg-deb -c` helpers for
//!     assertions on a produced .deb.
//!
//! Plus a `have(cmd)` PATH probe and a `skip_unless!` macro for tests
//! that need an external tool.

#![allow(dead_code)] // tests pick and choose; not every helper is used by every file

use std::ffi::OsStr;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Returns true if `cmd` is on PATH. Used by tests that shell out to
/// `dpkg-deb` and need to skip gracefully when it isn't installed.
pub fn have(cmd: &str) -> bool {
    Command::new("sh")
        .arg("-c")
        .arg(format!("command -v {} >/dev/null 2>&1", cmd))
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Skip the current `#[test]` (with an `eprintln!` note) when `cmd` is
/// not on PATH. Macro because it expands to an early `return` from the
/// test function.
#[macro_export]
macro_rules! skip_unless {
    ($cmd:literal) => {
        if !$crate::common::have($cmd) {
            eprintln!("skipping {}: `{}` not on PATH", module_path!(), $cmd);
            return;
        }
    };
}

// =============================================================================
// Toolkit: typed wrapper around the deb-toolkit binary
// =============================================================================

/// Wrapper around the `deb-toolkit` binary, with one method per session
/// verb. Each method returns a [`CmdOutput`] you call `.assert_success()`
/// (or `.assert_failure()`) on. Stdout from the process is available
/// via `.stdout` for `read-field` callers.
///
/// Construct with [`Toolkit::new`]; the binary path comes from
/// `CARGO_BIN_EXE_deb-toolkit` so it always matches the binary cargo
/// just built for this test target.
pub struct Toolkit {
    bin: PathBuf,
}

impl Toolkit {
    pub fn new() -> Self {
        Self {
            bin: PathBuf::from(env!("CARGO_BIN_EXE_deb-toolkit")),
        }
    }

    fn session<I, S>(&self, args: I) -> CmdOutput
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let mut cmd = Command::new(&self.bin);
        cmd.arg("session");
        cmd.args(args);
        CmdOutput::from(cmd.output().expect("spawn deb-toolkit"))
    }

    pub fn session_open(&self, input: &Path, session_dir: &Path) -> CmdOutput {
        self.session([
            OsStr::new("open"),
            input.as_os_str(),
            session_dir.as_os_str(),
        ])
    }

    pub fn session_save(&self, session_dir: &Path, output: &Path, verify: bool) -> CmdOutput {
        let mut args = vec![OsStr::new("save")];
        if verify {
            args.push(OsStr::new("--verify"));
        }
        args.push(session_dir.as_os_str());
        args.push(output.as_os_str());
        self.session(args)
    }

    pub fn session_read_field(&self, session_dir: &Path, field: &str) -> CmdOutput {
        self.session([
            OsStr::new("read-field"),
            session_dir.as_os_str(),
            OsStr::new(field),
        ])
    }

    pub fn session_rename_package(&self, session_dir: &Path, new_name: &str) -> CmdOutput {
        self.session([
            OsStr::new("rename-package"),
            session_dir.as_os_str(),
            OsStr::new(new_name),
        ])
    }

    pub fn session_replace_suite(&self, session_dir: &Path, new_suite: &str) -> CmdOutput {
        self.session([
            OsStr::new("replace-suite"),
            session_dir.as_os_str(),
            OsStr::new(new_suite),
        ])
    }

    pub fn session_reversion(
        &self,
        session_dir: &Path,
        new_version: &str,
        update_deps: bool,
    ) -> CmdOutput {
        let mut args = vec![OsStr::new("reversion")];
        if update_deps {
            args.push(OsStr::new("--update-deps"));
        }
        args.push(session_dir.as_os_str());
        args.push(OsStr::new(new_version));
        self.session(args)
    }

    pub fn session_insert(
        &self,
        session_dir: &Path,
        dest: &str,
        sources: &[&Path],
        as_directory: bool,
    ) -> CmdOutput {
        let mut args = vec![OsStr::new("insert")];
        if as_directory {
            args.push(OsStr::new("-d"));
        }
        args.push(session_dir.as_os_str());
        args.push(OsStr::new(dest));
        for s in sources {
            args.push(s.as_os_str());
        }
        self.session(args)
    }

    pub fn session_remove(&self, session_dir: &Path, pattern: &str) -> CmdOutput {
        self.session([
            OsStr::new("remove"),
            session_dir.as_os_str(),
            OsStr::new(pattern),
        ])
    }

    pub fn session_move(&self, session_dir: &Path, src: &str, dest: &str) -> CmdOutput {
        self.session([
            OsStr::new("move"),
            session_dir.as_os_str(),
            OsStr::new(src),
            OsStr::new(dest),
        ])
    }

    pub fn session_replace(
        &self,
        session_dir: &Path,
        pattern: &str,
        replacement: &Path,
    ) -> CmdOutput {
        self.session([
            OsStr::new("replace"),
            session_dir.as_os_str(),
            OsStr::new(pattern),
            replacement.as_os_str(),
        ])
    }

    pub fn session_apply(&self, session_dir: &Path, manifest: &Path) -> CmdOutput {
        self.session([
            OsStr::new("apply"),
            session_dir.as_os_str(),
            manifest.as_os_str(),
        ])
    }

    pub fn session_apply_dry_run(&self, session_dir: &Path, manifest: &Path) -> CmdOutput {
        self.session([
            OsStr::new("apply"),
            OsStr::new("--dry-run"),
            session_dir.as_os_str(),
            manifest.as_os_str(),
        ])
    }
}

impl Default for Toolkit {
    fn default() -> Self {
        Self::new()
    }
}

// =============================================================================
// CmdOutput: structured result of a binary invocation
// =============================================================================

/// Captured exit status + stdout + stderr from a `deb-toolkit` invocation.
pub struct CmdOutput {
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
}

impl CmdOutput {
    /// Panic with a useful message if the command failed.
    pub fn assert_success(self) -> Self {
        assert!(
            self.success,
            "command failed unexpectedly\nstdout:\n{}\nstderr:\n{}",
            self.stdout, self.stderr
        );
        self
    }

    /// Panic if the command unexpectedly *succeeded*. Returns self so
    /// callers can `.stderr_contains(...)` on the error output.
    pub fn assert_failure(self) -> Self {
        assert!(
            !self.success,
            "command unexpectedly succeeded\nstdout:\n{}",
            self.stdout
        );
        self
    }

    pub fn stdout_trim(&self) -> &str {
        self.stdout.trim()
    }

    /// Assert that stderr contains `needle` (handy for error-message
    /// checks). Returns self for chaining.
    pub fn stderr_contains(self, needle: &str) -> Self {
        assert!(
            self.stderr.contains(needle),
            "stderr missing {:?}:\n{}",
            needle,
            self.stderr
        );
        self
    }
}

impl From<std::process::Output> for CmdOutput {
    fn from(out: std::process::Output) -> Self {
        Self {
            success: out.status.success(),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        }
    }
}

// =============================================================================
// DebFixture: fluent builder for a fixture .deb
// =============================================================================

/// Fluent builder for a fixture `.deb` package. Replaces the inline
/// `fs::create_dir_all` + `fs::write(DEBIAN/control)` + `dpkg-deb -Zgzip
/// --build` boilerplate every test used to carry. The producer is
/// `dpkg-deb`, so [`skip_unless!`] on `"dpkg-deb"` is still required.
///
/// ```ignore
/// let deb = DebFixture::new("example-app")
///     .version("1.0.0")
///     .suite("stable")
///     .depends("example-helper (= 1.0.0), libssl3 (>= 3.0.0)")
///     .file("/var/lib/example/data.bin", b"original\n")
///     .file("/etc/example/config.json", b"{\"k\":1}\n")
///     .build(&tmp.path().join("example-app_1.0.0_amd64.deb"));
/// ```
pub struct DebFixture {
    name: String,
    version: String,
    arch: String,
    maintainer: String,
    description: String,
    suite: Option<String>,
    depends: Option<String>,
    files: Vec<(String, Vec<u8>)>,
    symlinks: Vec<(String, String)>,
    control_files: Vec<(String, Vec<u8>)>,
    modes: Vec<(String, u32)>,
    compression: String,
}

impl DebFixture {
    pub fn new(name: &str) -> Self {
        Self {
            name: name.into(),
            version: "1.0.0".into(),
            arch: "amd64".into(),
            maintainer: "test@example.com".into(),
            description: "fixture package".into(),
            suite: None,
            depends: None,
            files: Vec::new(),
            symlinks: Vec::new(),
            control_files: Vec::new(),
            modes: Vec::new(),
            compression: "gzip".into(),
        }
    }

    pub fn version(mut self, v: &str) -> Self {
        self.version = v.into();
        self
    }

    pub fn architecture(mut self, a: &str) -> Self {
        self.arch = a.into();
        self
    }

    pub fn maintainer(mut self, m: &str) -> Self {
        self.maintainer = m.into();
        self
    }

    pub fn description(mut self, d: &str) -> Self {
        self.description = d.into();
        self
    }

    pub fn suite(mut self, s: &str) -> Self {
        self.suite = Some(s.into());
        self
    }

    pub fn depends(mut self, d: &str) -> Self {
        self.depends = Some(d.into());
        self
    }

    /// Add a file at `pkg_path` (anchored at `/` inside the package).
    pub fn file(mut self, pkg_path: &str, content: impl Into<Vec<u8>>) -> Self {
        self.files.push((pkg_path.into(), content.into()));
        self
    }

    /// Add a symlink at `pkg_path` that points to `target`.
    pub fn symlink(mut self, pkg_path: &str, target: &str) -> Self {
        self.symlinks.push((pkg_path.into(), target.into()));
        self
    }

    /// Add a member to `DEBIAN/`, for example `postinst`.
    pub fn control_file(mut self, name: &str, content: impl Into<Vec<u8>>) -> Self {
        self.control_files.push((name.into(), content.into()));
        self
    }

    /// Set the mode of a file that was added with [`Self::file`].
    pub fn mode(mut self, pkg_path: &str, mode: u32) -> Self {
        self.modes.push((pkg_path.into(), mode));
        self
    }

    /// Set the `dpkg-deb -Z` compression. The default is `gzip`.
    pub fn compression(mut self, compression: &str) -> Self {
        self.compression = compression.into();
        self
    }

    /// Build the .deb at `out_path` using `dpkg-deb --build`.
    /// Panics on any failure — these are tests, so a precise panic
    /// message is more useful than an opaque Result chain.
    pub fn build(self, out_path: &Path) -> PathBuf {
        let staging = tempfile::tempdir().expect("tempdir");
        let pkg_root = staging.path().join("pkg");
        std::fs::create_dir_all(pkg_root.join("DEBIAN")).unwrap();

        let mut control = format!(
            "Package: {}\n\
             Version: {}\n\
             Architecture: {}\n\
             Maintainer: {}\n",
            self.name, self.version, self.arch, self.maintainer
        );
        if let Some(s) = &self.suite {
            control.push_str(&format!("Suite: {}\n", s));
        }
        if let Some(d) = &self.depends {
            control.push_str(&format!("Depends: {}\n", d));
        }
        control.push_str(&format!("Description: {}\n", self.description));
        std::fs::write(pkg_root.join("DEBIAN/control"), control).unwrap();

        for (pkg_path, content) in &self.files {
            let rel = pkg_path.trim_start_matches('/');
            let abs = pkg_root.join(rel);
            if let Some(parent) = abs.parent() {
                std::fs::create_dir_all(parent).unwrap();
            }
            std::fs::write(&abs, content).unwrap();
        }

        for (pkg_path, target) in &self.symlinks {
            let abs = pkg_root.join(pkg_path.trim_start_matches('/'));
            std::fs::create_dir_all(abs.parent().unwrap()).unwrap();
            std::os::unix::fs::symlink(target, abs).unwrap();
        }

        for (name, content) in &self.control_files {
            let abs = pkg_root.join("DEBIAN").join(name);
            std::fs::write(&abs, content).unwrap();
            let executable = std::fs::Permissions::from_mode(0o755);
            std::fs::set_permissions(&abs, executable).unwrap();
        }

        for (pkg_path, mode) in &self.modes {
            let abs = pkg_root.join(pkg_path.trim_start_matches('/'));
            std::fs::set_permissions(abs, std::fs::Permissions::from_mode(*mode)).unwrap();
        }

        let out = Command::new("dpkg-deb")
            .arg(format!("-Z{}", self.compression))
            .args(["--root-owner-group", "--build"])
            .arg(&pkg_root)
            .arg(out_path)
            .output()
            .expect("dpkg-deb --build");
        assert!(
            out.status.success(),
            "dpkg-deb --build failed:\n{}",
            String::from_utf8_lossy(&out.stderr)
        );
        out_path.to_path_buf()
    }
}

// =============================================================================
// dpkg: assertion helpers
// =============================================================================

pub mod dpkg {
    use super::*;

    /// `dpkg-deb --info <deb>`. Panics on non-zero exit.
    pub fn info(deb: &Path) -> String {
        let out = Command::new("dpkg-deb")
            .arg("--info")
            .arg(deb)
            .output()
            .expect("dpkg-deb --info");
        assert!(
            out.status.success(),
            "dpkg-deb --info failed:\n{}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    /// `dpkg-deb -c <deb>`. Panics on non-zero exit.
    pub fn contents(deb: &Path) -> String {
        let out = Command::new("dpkg-deb")
            .arg("-c")
            .arg(deb)
            .output()
            .expect("dpkg-deb -c");
        assert!(
            out.status.success(),
            "dpkg-deb -c failed:\n{}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).into_owned()
    }
}
