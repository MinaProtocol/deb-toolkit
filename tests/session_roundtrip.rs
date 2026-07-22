//! Verb-by-verb roundtrip test for the session subsystem.
//!
//! Builds a fixture .deb, opens it, runs every session verb against
//! it in sequence (read-field, rename-package, replace-suite,
//! reversion --update-deps, insert single, insert -d, replace via
//! glob, move, remove via glob), saves with --verify, and asserts on
//! `dpkg-deb --info` and `dpkg-deb -c` output that each mutation
//! actually took effect.
//!
//! The companion `session_variant_scenario.rs` covers the *combined*
//! workflow (rebrand a release as a variant) rather than individual
//! verbs. This file is the per-verb coverage net.
//!
//! Skipped when `dpkg-deb` isn't on PATH.

mod common;
use common::*;

#[test]
fn session_full_roundtrip() {
    skip_unless!("dpkg-deb");

    let tk = Toolkit::new();
    let tmp = tempfile::tempdir().unwrap();

    // Fixture: a small package with a couple of config files and a
    // file that will be moved across the session.
    let input = DebFixture::new("session-fixture")
        .version("1.0.0")
        .suite("unstable")
        .depends("libfoo (= 1.0.0), libbar (>= 1.0.0), libbaz")
        .file(
            "/usr/share/test/configs/config_devnet.json",
            b"{\"network\":\"devnet\"}\n".to_vec(),
        )
        .file(
            "/usr/share/test/configs/config_mainnet.json",
            b"{\"network\":\"mainnet\"}\n".to_vec(),
        )
        .file("/usr/share/test/keep-me.txt", b"keep this\n".to_vec())
        .build(&tmp.path().join("session-fixture_1.0.0_amd64.deb"));

    // session open
    let session = tmp.path().join("session");
    tk.session_open(&input, &session).assert_success();
    assert!(session.join("metadata.env").is_file());
    assert!(session.join("control/control").is_file());
    assert!(session.join("data").is_dir());

    // read-field
    let read = tk.session_read_field(&session, "Package").assert_success();
    assert_eq!(read.stdout_trim(), "session-fixture");

    // rename-package + replace-suite + reversion --update-deps
    tk.session_rename_package(&session, "session-renamed")
        .assert_success();
    tk.session_replace_suite(&session, "stable")
        .assert_success();
    tk.session_reversion(&session, "2.0.0", /*update_deps=*/ true)
        .assert_success();

    // insert single
    let new_file = tmp.path().join("data.bin");
    std::fs::write(&new_file, b"fake data contents\n").unwrap();
    tk.session_insert(
        &session,
        "/var/lib/example/data.bin",
        &[&new_file],
        /*directory=*/ false,
    )
    .assert_success();
    assert!(session.join("data/var/lib/example/data.bin").is_file());

    // insert -d (multi → directory)
    let f1 = tmp.path().join("f1.bin");
    let f2 = tmp.path().join("f2.bin");
    std::fs::write(&f1, b"1\n").unwrap();
    std::fs::write(&f2, b"2\n").unwrap();
    tk.session_insert(
        &session,
        "/var/lib/example",
        &[&f1, &f2],
        /*directory=*/ true,
    )
    .assert_success();
    assert!(session.join("data/var/lib/example/f1.bin").is_file());

    // replace via glob (overwrites both config_*.json files)
    let new_config = tmp.path().join("new_config.json");
    std::fs::write(&new_config, b"{\"new\": true}\n").unwrap();
    tk.session_replace(
        &session,
        "/usr/share/test/configs/config_*.json",
        &new_config,
    )
    .assert_success();
    let after =
        std::fs::read_to_string(session.join("data/usr/share/test/configs/config_devnet.json"))
            .unwrap();
    assert!(after.contains("\"new\": true"));

    // move
    tk.session_move(
        &session,
        "/usr/share/test/keep-me.txt",
        "/usr/share/test/moved.txt",
    )
    .assert_success();
    assert!(!session.join("data/usr/share/test/keep-me.txt").exists());
    assert!(session.join("data/usr/share/test/moved.txt").is_file());

    // remove via glob (deletes both config_*.json)
    tk.session_remove(&session, "/usr/share/test/configs/config_*.json")
        .assert_success();
    assert!(!session
        .join("data/usr/share/test/configs/config_devnet.json")
        .exists());

    // save with --verify
    let output = tmp.path().join("output.deb");
    tk.session_save(&session, &output, /*verify=*/ true)
        .assert_success();
    assert!(output.is_file());

    // Assertions on the produced .deb.
    let info = dpkg::info(&output);
    assert!(
        info.contains("Package: session-renamed"),
        "Package not renamed:\n{}",
        info
    );
    assert!(
        info.contains("Version: 2.0.0"),
        "Version not bumped:\n{}",
        info
    );
    assert!(info.contains("Suite: stable"), "Suite not set:\n{}", info);
    // Reversion rewrites the pinned version for EVERY relation operator, not
    // just `=`: a reversion can lower the version, so a stale `(>= old)` would
    // be unsatisfiable. Both constraints track the new version.
    assert!(
        info.contains("libfoo (= 2.0.0)"),
        "= pin not rewritten:\n{}",
        info
    );
    assert!(
        info.contains("libbar (>= 2.0.0)"),
        ">= constraint not rewritten:\n{}",
        info
    );

    let contents = dpkg::contents(&output);
    assert!(
        contents.contains("var/lib/example/data.bin"),
        "{}",
        contents
    );
    assert!(contents.contains("var/lib/example/f1.bin"), "{}", contents);
    assert!(
        contents.contains("usr/share/test/moved.txt"),
        "{}",
        contents
    );
    assert!(
        !contents.contains("usr/share/test/keep-me.txt"),
        "{}",
        contents
    );
    assert!(
        !contents.contains("usr/share/test/configs/config_devnet.json"),
        "{}",
        contents
    );
}
