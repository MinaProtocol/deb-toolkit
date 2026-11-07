//! End-to-end tests for `deb-toolkit diff`, on real packages from `dpkg-deb`.
//!
//! The rules at unit level live in `src/diff.rs`. These tests prove that the
//! rules reach the command line: exit status, report text, JSON, and the
//! `.deb` reader for each compression format.
//!
//! Skipped when `dpkg-deb` isn't on PATH.

mod common;
use common::*;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn diff(old: &Path, new: &Path, extra: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_deb-toolkit"))
        .arg("diff")
        .arg(old)
        .arg(new)
        .args(extra)
        .output()
        .expect("spawn deb-toolkit")
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn fixture() -> DebFixture {
    DebFixture::new("example")
        .version("1.0.0")
        .depends("liba (>= 1.0), libb")
        .file("/usr/bin/tool", b"binary\n".to_vec())
        .file("/etc/example/config", b"value=1\n".to_vec())
        .control_file("postinst", b"#!/bin/sh\necho hello\n".to_vec())
}

struct Pair {
    _dir: tempfile::TempDir,
    old: PathBuf,
    new: PathBuf,
}

fn pair(old: DebFixture, new: DebFixture) -> Pair {
    let dir = tempfile::tempdir().unwrap();
    let old = old.build(&dir.path().join("old.deb"));
    let new = new.build(&dir.path().join("new.deb"));
    Pair {
        _dir: dir,
        old,
        new,
    }
}

#[test]
fn a_rebuild_of_the_same_input_matches() {
    skip_unless!("dpkg-deb");
    let p = pair(fixture(), fixture());
    let out = diff(&p.old, &p.new, &[]);
    assert_eq!(out.status.code(), Some(0), "{}", stdout(&out));
    assert!(stdout(&out).contains("Result: packages match"));
}

#[test]
fn compression_is_a_note_and_every_format_can_be_read() {
    skip_unless!("dpkg-deb");
    for format in ["xz", "zstd", "none"] {
        let p = pair(fixture(), fixture().compression(format));
        let out = diff(&p.old, &p.new, &[]);
        assert_eq!(out.status.code(), Some(0), "{}: {}", format, stdout(&out));
        assert!(
            stdout(&out).contains("data compression: gz -> "),
            "{}: {}",
            format,
            stdout(&out)
        );
    }
}

#[test]
fn file_content_change_is_reported_with_exit_1() {
    skip_unless!("dpkg-deb");
    let new = DebFixture::new("example")
        .version("1.0.0")
        .depends("liba (>= 1.0), libb")
        .file("/usr/bin/tool", b"binary v2\n".to_vec())
        .file("/etc/example/config", b"value=1\n".to_vec())
        .control_file("postinst", b"#!/bin/sh\necho hello\n".to_vec());
    let p = pair(fixture(), new);
    let out = diff(&p.old, &p.new, &[]);
    assert_eq!(out.status.code(), Some(1));
    let text = stdout(&out);
    assert!(text.contains("~ /usr/bin/tool"), "{}", text);
    assert!(text.contains("content: 7 bytes"), "{}", text);
}

#[test]
fn added_removed_mode_and_symlink_changes_appear_in_json() {
    skip_unless!("dpkg-deb");
    let old = fixture()
        .file("/usr/lib/removed", b"x".to_vec())
        .symlink("/usr/bin/link", "tool");
    let new = fixture()
        .file("/usr/lib/added", b"x".to_vec())
        .mode("/etc/example/config", 0o600)
        .symlink("/usr/bin/link", "other");
    let p = pair(old, new);
    let out = diff(&p.old, &p.new, &["--json"]);
    assert_eq!(out.status.code(), Some(1));

    let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["identical"], false);
    let files = report["files"].as_array().unwrap();
    let find = |path: &str| {
        files
            .iter()
            .find(|f| f["path"] == path)
            .unwrap_or_else(|| panic!("{} missing from {:#?}", path, files))
    };
    assert_eq!(find("/usr/lib/removed")["kind"], "removed");
    assert_eq!(find("/usr/lib/added")["kind"], "added");
    assert_eq!(
        find("/etc/example/config")["changes"][0]["attribute"],
        "mode"
    );
    assert_eq!(find("/usr/bin/link")["changes"][0]["attribute"], "link");
}

#[test]
fn dependency_order_does_not_matter_but_content_does() {
    skip_unless!("dpkg-deb");
    let reordered = DebFixture::new("example")
        .version("1.0.0")
        .depends("libb, liba (>=1.0)")
        .file("/usr/bin/tool", b"binary\n".to_vec())
        .file("/etc/example/config", b"value=1\n".to_vec())
        .control_file("postinst", b"#!/bin/sh\necho hello\n".to_vec());
    let p = pair(fixture(), reordered);
    assert_eq!(diff(&p.old, &p.new, &[]).status.code(), Some(0));

    let changed = DebFixture::new("example")
        .version("1.0.0")
        .depends("liba (>= 2.0), libb")
        .file("/usr/bin/tool", b"binary\n".to_vec())
        .file("/etc/example/config", b"value=1\n".to_vec())
        .control_file("postinst", b"#!/bin/sh\necho hello\n".to_vec());
    let p = pair(fixture(), changed);
    let out = diff(&p.old, &p.new, &[]);
    assert_eq!(out.status.code(), Some(1));
    let text = stdout(&out);
    assert!(text.contains("- liba (>= 1.0)"), "{}", text);
    assert!(text.contains("+ liba (>= 2.0)"), "{}", text);
}

#[test]
fn a_maintainer_script_change_shows_a_line_diff() {
    skip_unless!("dpkg-deb");
    let new = DebFixture::new("example")
        .version("1.0.0")
        .depends("liba (>= 1.0), libb")
        .file("/usr/bin/tool", b"binary\n".to_vec())
        .file("/etc/example/config", b"value=1\n".to_vec())
        .control_file("postinst", b"#!/bin/sh\necho goodbye\n".to_vec())
        .control_file("prerm", b"#!/bin/sh\n".to_vec());
    let p = pair(fixture(), new);
    let out = diff(&p.old, &p.new, &[]);
    assert_eq!(out.status.code(), Some(1));
    let text = stdout(&out);
    assert!(text.contains("-echo hello"), "{}", text);
    assert!(text.contains("+echo goodbye"), "{}", text);
    assert!(text.contains("+ prerm"), "{}", text);
}

#[test]
fn ignore_flags_remove_only_what_they_name() {
    skip_unless!("dpkg-deb");
    let new = DebFixture::new("example")
        .version("2.0.0")
        .depends("liba (>= 1.0), libb")
        .file("/usr/bin/tool", b"binary\n".to_vec())
        .file("/etc/example/config", b"value=2\n".to_vec())
        .control_file("postinst", b"#!/bin/sh\necho hello\n".to_vec());
    let p = pair(fixture(), new);

    assert_eq!(diff(&p.old, &p.new, &[]).status.code(), Some(1));
    let only_field = diff(&p.old, &p.new, &["--ignore-field", "version"]);
    assert_eq!(only_field.status.code(), Some(1));
    let only_path = diff(&p.old, &p.new, &["--ignore-path", "/etc/*"]);
    assert_eq!(only_path.status.code(), Some(1));
    let both = diff(
        &p.old,
        &p.new,
        &["--ignore-field", "Version", "--ignore-path", "/etc/*"],
    );
    assert_eq!(both.status.code(), Some(0), "{}", stdout(&both));
}

#[test]
fn a_session_roundtrip_changes_nothing_that_a_user_gets() {
    skip_unless!("dpkg-deb");
    let tk = Toolkit::new();
    let dir = tempfile::tempdir().unwrap();
    let old = fixture()
        .symlink("/usr/bin/link", "tool")
        .build(&dir.path().join("old.deb"));
    let session = dir.path().join("session");
    let new = dir.path().join("new.deb");
    tk.session_open(&old, &session).assert_success();
    tk.session_save(&session, &new, false).assert_success();

    let out = diff(&old, &new, &[]);
    assert_eq!(out.status.code(), Some(0), "{}", stdout(&out));
}

#[test]
fn bad_input_exits_with_2() {
    skip_unless!("dpkg-deb");
    let dir = tempfile::tempdir().unwrap();
    let good = fixture().build(&dir.path().join("good.deb"));
    let text = dir.path().join("text.deb");
    std::fs::write(&text, "not an archive").unwrap();

    assert_eq!(diff(&good, &text, &[]).status.code(), Some(2));
    assert_eq!(
        diff(&good, &dir.path().join("missing.deb"), &[])
            .status
            .code(),
        Some(2)
    );
    assert_eq!(
        diff(&good, &good, &["--ignore-path", "["]).status.code(),
        Some(2)
    );
}
