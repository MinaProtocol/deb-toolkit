//! `deb-toolkit build` must give the same package as the bash builder in the
//! mina repo (`create_control_file` and `build_deb` in
//! `scripts/debian/builder-helpers.sh`).
//!
//! Each test writes the stanza that the bash builder writes, runs `build` on the
//! same files, and compares the control fields. `Installed-Size` is the one
//! expected difference: bash leaves it blank, and `build` computes it.
//!
//! Skipped when `dpkg-deb` or `fakeroot` isn't on PATH.

mod common;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const PACKAGE: &str = "mina-example";
const VERSION: &str = "3.1.0-abc1234";
const BIN: &str = env!("CARGO_BIN_EXE_deb-toolkit");

/// The arguments of `create_control_file`, without the package name.
#[derive(Clone, Copy, Default)]
struct Control<'a> {
    depends: &'a str,
    suggests: &'a str,
    /// The bash builder writes this value to both `Replaces` and `Breaks`.
    replaces_breaks: &'a str,
    provides: &'a str,
    conflicts: &'a str,
}

/// The stanza from `create_control_file`, in the same field order.
fn bash_stanza(c: &Control) -> String {
    let mut out = format!(
        "Package: {PACKAGE}\nVersion: {VERSION}\nLicense: Apache-2.0\n\
         Origin: MinaProtocol\nLabel: MinaProtocol\nVendor: O(1)Labs\n\
         Codename: bullseye\nArchitecture: amd64\n\
         Maintainer: O(1)Labs <build@o1labs.org>\nInstalled-Size:\n"
    );
    for (name, value) in [
        ("Depends", c.depends),
        ("Suggests", c.suggests),
        ("Replaces", c.replaces_breaks),
        ("Breaks", c.replaces_breaks),
        ("Provides", c.provides),
        ("Conflicts", c.conflicts),
    ] {
        if !value.is_empty() {
            out.push_str(&format!("{name}: {value}\n"));
        }
    }
    out.push_str(
        "Section: base\nPriority: optional\nHomepage: https://minaprotocol.com/\n\
         Description:\n Example package\n Built from abc1234 by https://ci.example/1\n",
    );
    out
}

/// The files of the package. A metapackage has none.
#[derive(Clone, Copy)]
enum Files {
    Some,
    None,
}

fn stage(dir: &Path, files: Files) {
    std::fs::create_dir_all(dir).unwrap();
    if let Files::Some = files {
        let bin = dir.join("usr/local/bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(bin.join("tool"), b"binary\n").unwrap();
    }
}

/// `fakeroot dpkg-deb -Zgzip --build`, as in `build_deb`.
fn bash_build(root: &Path, stanza: &str, files: Files) -> PathBuf {
    let tree = root.join("bash-tree");
    stage(&tree, files);
    std::fs::create_dir_all(tree.join("DEBIAN")).unwrap();
    std::fs::write(tree.join("DEBIAN/control"), stanza).unwrap();

    let deb = root.join("bash.deb");
    let status = Command::new("fakeroot")
        .args(["dpkg-deb", "-Zgzip", "--build"])
        .arg(&tree)
        .arg(&deb)
        .output()
        .unwrap()
        .status;
    assert!(status.success(), "dpkg-deb failed");
    deb
}

/// The flags that hold what `create_control_file` hard-codes.
fn fixed_flags() -> Vec<String> {
    [
        ("--license", "Apache-2.0"),
        ("--origin", "MinaProtocol"),
        ("--label", "MinaProtocol"),
        ("--vendor", "O(1)Labs"),
        ("--architecture", "amd64"),
        ("--maintainer", "O(1)Labs <build@o1labs.org>"),
        ("--section", "base"),
        ("--priority", "optional"),
        ("--homepage", "https://minaprotocol.com/"),
        ("--description", "Example package"),
        ("--githash", "abc1234"),
        ("--buildurl", "https://ci.example/1"),
    ]
    .iter()
    .flat_map(|(flag, value)| [flag.to_string(), value.to_string()])
    .collect()
}

fn relation_flags(c: &Control) -> Vec<String> {
    [
        ("--depends", c.depends),
        ("--suggested-depends", c.suggests),
        ("--replaces", c.replaces_breaks),
        ("--breaks", c.replaces_breaks),
        ("--provides", c.provides),
        ("--conflicts", c.conflicts),
    ]
    .iter()
    .filter(|(_, value)| !value.is_empty())
    .flat_map(|(flag, value)| [flag.to_string(), value.to_string()])
    .collect()
}

fn toolkit_build(root: &Path, flags: &[String], files: Files) -> PathBuf {
    let tree = root.join("toolkit-tree");
    stage(&tree, files);
    let out = root.join("out");

    let output = Command::new(BIN)
        .arg("build")
        .arg("--build-dir")
        .arg(&tree)
        .arg("--output-dir")
        .arg(&out)
        .args(["--package-name", PACKAGE, "--version", VERSION])
        .args(["--codename", "bullseye"])
        .args(flags)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "build failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    out.join(format!("{PACKAGE}_{VERSION}_amd64.deb"))
}

/// The control fields of a package, without `Installed-Size`.
fn fields(deb: &Path) -> BTreeMap<String, String> {
    let output = Command::new("dpkg-deb")
        .arg("-f")
        .arg(deb)
        .output()
        .unwrap();
    assert!(output.status.success(), "dpkg-deb -f failed");

    let text = String::from_utf8(output.stdout).unwrap();
    let mut fields: BTreeMap<String, String> = BTreeMap::new();
    let mut current = String::new();
    for line in text.lines() {
        if line.starts_with(' ') {
            let value = fields.get_mut(&current).unwrap();
            value.push('\n');
            value.push_str(line.trim());
        } else {
            let (name, value) = line.split_once(':').unwrap();
            current = name.to_string();
            fields.insert(current.clone(), value.trim().to_string());
        }
    }
    fields.remove("Installed-Size");
    fields
}

fn field(deb: &Path, name: &str) -> String {
    let output = Command::new("dpkg-deb")
        .arg("-f")
        .arg(deb)
        .arg(name)
        .output()
        .unwrap();
    String::from_utf8(output.stdout).unwrap().trim().to_string()
}

fn verify(deb: &Path, extra: &[&str]) -> Output {
    Command::new(BIN)
        .args(["verify", "content", "--deb"])
        .arg(deb)
        .args(extra)
        .output()
        .unwrap()
}

/// Each shape of a real `create_control_file` call in the mina repo.
fn shapes() -> Vec<(&'static str, Control<'static>)> {
    let shared = "libssl1.1, libgmp10, libgomp1, tzdata, liblmdb0";
    vec![
        ("no dependencies", Control::default()),
        (
            "depends",
            Control {
                depends: shared,
                ..Default::default()
            },
        ),
        (
            "trailing space, from TEST_EXECUTIVE_DEPS",
            Control {
                depends: "libgmp10, mina-logproc, python3, docker-ce ",
                ..Default::default()
            },
        ),
        (
            "suggests, from the daemon packages",
            Control {
                depends: "libgmp10, mina-devnet-profile (=3.1.0-abc1234)",
                suggests: "jq, curl, wget",
                ..Default::default()
            },
        ),
        (
            "replaces and breaks, from mina-tx-tools",
            Control {
                depends: shared,
                replaces_breaks:
                    "mina-batch-txn (<< 3.1.0), mina-zkapp-test-transaction (<< 3.1.0)",
                ..Default::default()
            },
        ),
        (
            "provides, from a flavored mina-generic",
            Control {
                depends: shared,
                suggests: "jq, curl, wget",
                provides: "mina-generic (= 3.1.0-abc1234)",
                ..Default::default()
            },
        ),
        (
            "conflicts, from the network metapackage",
            Control {
                depends: "mina-devnet-generic (=3.1.0)",
                conflicts: "mina-devnet-automode",
                ..Default::default()
            },
        ),
        (
            "all fields, from the automode metapackage",
            Control {
                depends: "mina-devnet-postfork (=3.1.0)",
                replaces_breaks: "mina-devnet, mina-generic",
                provides: "mina-devnet",
                conflicts: "mina-devnet, mina-generic",
                ..Default::default()
            },
        ),
    ]
}

#[test]
fn build_matches_the_bash_builder_for_each_control_shape() {
    skip_unless!("dpkg-deb");
    skip_unless!("fakeroot");

    for (name, control) in shapes() {
        let dir = tempfile::tempdir().unwrap();
        let old = bash_build(dir.path(), &bash_stanza(&control), Files::Some);
        let mut flags = fixed_flags();
        flags.extend(relation_flags(&control));
        let new = toolkit_build(dir.path(), &flags, Files::Some);

        assert_eq!(fields(&old), fields(&new), "{}", name);
    }
}

/// A metapackage (a package with no files, only control fields) is built by
/// the bash builder for the network and automode packages.
#[test]
fn build_matches_the_bash_builder_for_a_metapackage() {
    skip_unless!("dpkg-deb");
    skip_unless!("fakeroot");

    let control = Control {
        depends: "mina-devnet-config (=3.1.0-abc1234), mina-devnet-profile",
        ..Default::default()
    };
    let dir = tempfile::tempdir().unwrap();
    let old = bash_build(dir.path(), &bash_stanza(&control), Files::None);
    let mut flags = fixed_flags();
    flags.extend(relation_flags(&control));
    let new = toolkit_build(dir.path(), &flags, Files::None);

    assert_eq!(fields(&old), fields(&new));
    let contents = Command::new("dpkg-deb")
        .arg("-c")
        .arg(&new)
        .output()
        .unwrap();
    assert!(contents.status.success());
    assert_eq!(
        String::from_utf8_lossy(&contents.stdout).lines().count(),
        1,
        "only the root directory"
    );
}

/// The parity test above is worth nothing if it cannot see these fields.
#[test]
fn each_flag_is_needed_for_parity() {
    skip_unless!("dpkg-deb");
    skip_unless!("fakeroot");

    let control = Control {
        depends: "libgmp10",
        replaces_breaks: "mina-old (<< 3.1.0)",
        ..Default::default()
    };
    let mut full = fixed_flags();
    full.extend(relation_flags(&control));

    for flag in ["--origin", "--label", "--breaks"] {
        let at = full.iter().position(|f| f == flag).unwrap();
        let mut flags = full.clone();
        flags.drain(at..at + 2);

        let dir = tempfile::tempdir().unwrap();
        let old = bash_build(dir.path(), &bash_stanza(&control), Files::Some);
        let new = toolkit_build(dir.path(), &flags, Files::Some);

        assert_ne!(fields(&old), fields(&new), "without {}", flag);
    }
}

#[test]
fn verify_content_accepts_the_output_of_build() {
    skip_unless!("dpkg-deb");
    skip_unless!("fakeroot");

    let control = Control {
        replaces_breaks: "mina-old (<< 3.1.0)",
        ..Default::default()
    };
    let mut flags = fixed_flags();
    flags.extend(relation_flags(&control));
    let dir = tempfile::tempdir().unwrap();
    let deb = toolkit_build(dir.path(), &flags, Files::Some);

    let good = [
        "--architecture",
        "amd64",
        "--origin",
        "MinaProtocol",
        "--label",
        "MinaProtocol",
        "--breaks",
        "mina-old (<< 3.1.0)",
    ];
    let output = verify(&deb, &good);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    for bad in [
        ["--origin", "Other"],
        ["--label", "Other"],
        ["--breaks", "other"],
    ] {
        assert!(!verify(&deb, &bad).status.success(), "{:?} passed", bad);
    }
}

/// `Authors` and `Source` are absent, so a blank default must not fail.
#[test]
fn verify_content_accepts_blank_defaults_for_omitted_fields() {
    skip_unless!("dpkg-deb");
    skip_unless!("fakeroot");

    let dir = tempfile::tempdir().unwrap();
    let deb = toolkit_build(dir.path(), &fixed_flags(), Files::Some);
    let defaults = dir.path().join("defaults.json");
    std::fs::write(
        &defaults,
        r#"{"package_source": "", "package_authors": "", "package_installed_size": ""}"#,
    )
    .unwrap();

    let output = verify(&deb, &["--defaults-file", defaults.to_str().unwrap()]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let output = verify(&deb, &["--source", "mina"]);
    assert!(
        !output.status.success(),
        "a set value must still be checked"
    );
}

#[test]
fn verify_content_checks_the_architecture_in_the_file_name() {
    skip_unless!("dpkg-deb");
    skip_unless!("fakeroot");

    let dir = tempfile::tempdir().unwrap();
    let deb = toolkit_build(dir.path(), &fixed_flags(), Files::Some);

    let renamed = dir.path().join(format!("{PACKAGE}_{VERSION}_arm64.deb"));
    std::fs::copy(&deb, &renamed).unwrap();
    assert!(!verify(&renamed, &[]).status.success());

    let old_name = dir.path().join(format!("{PACKAGE}_{VERSION}.deb"));
    std::fs::copy(&deb, &old_name).unwrap();
    assert!(verify(&old_name, &[]).status.success());
}

/// A migrated build keeps `Origin` and `Label` in a defaults file, so both
/// `build` and `verify content` must read the new fields from there.
#[test]
fn origin_label_and_breaks_work_from_a_defaults_file() {
    skip_unless!("dpkg-deb");
    skip_unless!("fakeroot");

    let control = Control {
        replaces_breaks: "mina-old (<< 3.1.0)",
        ..Default::default()
    };
    let dir = tempfile::tempdir().unwrap();
    let defaults = dir.path().join("defaults.json");
    std::fs::write(
        &defaults,
        r#"{"origin": "MinaProtocol", "label": "MinaProtocol",
            "breaks": ["mina-old (<< 3.1.0)"]}"#,
    )
    .unwrap();

    // The defaults supply these three, so remove their flags.
    let mut flags = fixed_flags();
    for flag in ["--origin", "--label"] {
        let at = flags.iter().position(|f| f == flag).unwrap();
        flags.drain(at..at + 2);
    }
    flags.extend([
        "--replaces".to_string(),
        control.replaces_breaks.to_string(),
    ]);
    let mut with_defaults = flags.clone();
    with_defaults.extend([
        "--defaults-file".to_string(),
        defaults.to_str().unwrap().to_string(),
    ]);

    let old = bash_build(dir.path(), &bash_stanza(&control), Files::Some);
    let new = toolkit_build(dir.path(), &with_defaults, Files::Some);
    assert_eq!(fields(&old), fields(&new));

    let args = ["--defaults-file", defaults.to_str().unwrap()];
    assert!(verify(&new, &args).status.success());

    // Each field alone: a package that lacks it must fail against the defaults.
    for (field, value) in [
        ("origin", r#""MinaProtocol""#),
        ("label", r#""MinaProtocol""#),
        ("breaks", r#"["mina-old (<< 3.1.0)"]"#),
    ] {
        let one = dir.path().join(format!("{field}.json"));
        std::fs::write(&one, format!(r#"{{"{field}": {value}}}"#)).unwrap();

        let plain = dir.path().join("plain");
        std::fs::create_dir_all(&plain).unwrap();
        let bare = toolkit_build(&plain, &flags, Files::Some);
        let args = ["--defaults-file", one.to_str().unwrap()];
        assert!(!verify(&bare, &args).status.success(), "{}", field);
        std::fs::remove_dir_all(&plain).unwrap();
    }
}

#[test]
fn installed_size_is_computed_unless_a_value_is_given() {
    skip_unless!("dpkg-deb");
    skip_unless!("fakeroot");

    // The tree of `stage`: root, usr, local, bin, and one small file.
    let computed = "5";

    let dir = tempfile::tempdir().unwrap();
    let deb = toolkit_build(dir.path(), &fixed_flags(), Files::Some);
    assert_eq!(field(&deb, "Installed-Size"), computed);

    // A blank value means unset, as in `packaging/defaults/toolkit.json`.
    let dir = tempfile::tempdir().unwrap();
    let mut flags = fixed_flags();
    flags.extend(["--installed-size".to_string(), String::new()]);
    let deb = toolkit_build(dir.path(), &flags, Files::Some);
    assert_eq!(field(&deb, "Installed-Size"), computed);

    let dir = tempfile::tempdir().unwrap();
    let mut flags = fixed_flags();
    flags.extend(["--installed-size".to_string(), "1234".to_string()]);
    let deb = toolkit_build(dir.path(), &flags, Files::Some);
    assert_eq!(field(&deb, "Installed-Size"), "1234");

    assert!(verify(&deb, &["--installed-size", "1234"]).status.success());
    assert!(!verify(&deb, &["--installed-size", "99"]).status.success());
}
