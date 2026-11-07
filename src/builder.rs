use anyhow::{anyhow, Context, Result};
use std::collections::HashSet;
use std::os::unix::fs::MetadataExt;
use std::path::Path;

use crate::cli::{BuildArgs, OptionalMetadataArgs};
use crate::deb_name::DebName;
use crate::defaults::Defaults;
use crate::templates::{format_control_file, DebianControl};

/// Fully-resolved build input: all required fields have a concrete value,
/// either from the CLI or from the defaults file.
#[derive(Debug, Clone)]
pub struct Input {
    pub build_dir: String,
    pub output_dir: String,
    pub clean: bool,
    pub package_name: String,
    pub version: String,
    pub vendor: String,
    pub origin: String,
    pub label: String,
    pub package_authors: String,
    pub package_maintainer: String,
    pub package_description: String,
    pub package_section: String,
    pub package_priority: String,
    pub package_homepage: String,
    pub package_installed_size: String,
    pub package_source: String,
    pub architecture: String,
    pub suite: String,
    pub codename: String,
    pub depends: Option<Vec<String>>,
    pub suggested_depends: Option<Vec<String>>,
    pub recommended_depends: Option<Vec<String>>,
    pub pre_depends: Option<Vec<String>>,
    pub conflicts: Option<Vec<String>>,
    pub replaces: Option<Vec<String>>,
    pub breaks: Option<Vec<String>>,
    pub provides: Option<Vec<String>>,
    pub license: String,
    pub githash: String,
    pub buildurl: String,
    pub debug: bool,
}

fn split_csv(s: &Option<String>) -> Option<Vec<String>> {
    s.as_ref()
        .map(|raw| raw.split(',').map(|s| s.to_string()).collect())
}

fn merge_list(cli: Option<Vec<String>>, defaults: Option<Vec<String>>) -> Option<Vec<String>> {
    match (cli, defaults) {
        (Some(a), Some(b)) => {
            let mut out = a;
            out.extend(b);
            Some(out)
        }
        (Some(a), None) => Some(a),
        (None, Some(b)) => Some(b),
        (None, None) => None,
    }
}

/// Pick a value from CLI ⊕ defaults, returning a useful error if neither has it.
fn pick(
    cli: Option<String>,
    default: Option<String>,
    name: &str,
    defaults_file: Option<&str>,
) -> Result<String> {
    if let Some(v) = cli {
        return Ok(v);
    }
    if let Some(v) = default {
        return Ok(v);
    }
    Err(match defaults_file {
        None => anyhow!("{} not defined in cli", name),
        Some(p) => anyhow!("{} not defined in defaults file ({}) nor in cli", name, p),
    })
}

/// Pick a value that may stay unset. The template decides what a blank value
/// means for each field.
fn pick_optional(cli: Option<String>, default: Option<String>) -> String {
    cli.or(default).unwrap_or_default()
}

pub fn evaluate_and_validate(args: &BuildArgs) -> Result<Input> {
    let defaults = Defaults::load(args.defaults_file.as_deref())?;

    if !std::path::Path::new(&args.build_dir).exists() {
        anyhow::bail!("Build directory does not exist: {}", args.build_dir);
    }

    let m: &OptionalMetadataArgs = &args.metadata;
    let df = args.defaults_file.as_deref();

    let depends = merge_list(split_csv(&m.depends), defaults.depends);
    let suggested_depends = merge_list(split_csv(&m.suggested_depends), defaults.suggested_depends);
    let recommended_depends = merge_list(
        split_csv(&m.recommended_depends),
        defaults.recommended_depends,
    );
    let pre_depends = merge_list(split_csv(&m.pre_depends), defaults.pre_depends);
    let conflicts = merge_list(split_csv(&m.conflicts), defaults.conflicts);
    let replaces = merge_list(split_csv(&m.replaces), defaults.replaces);
    let breaks = merge_list(split_csv(&m.breaks), defaults.breaks);
    let provides = merge_list(split_csv(&m.provides), defaults.provides);

    let vendor = pick(m.vendor.clone(), defaults.vendor, "vendor", df)?;
    let origin = pick_optional(m.origin.clone(), defaults.origin);
    let label = pick_optional(m.label.clone(), defaults.label);
    let package_authors = pick_optional(m.authors.clone(), defaults.package_authors);
    let package_maintainer = pick(
        m.maintainer.clone(),
        defaults.package_maintainer,
        "package_maintainer",
        df,
    )?;
    let package_section = pick(
        m.section.clone(),
        defaults.package_section,
        "package_section",
        df,
    )?;
    let package_priority = pick(
        m.priority.clone(),
        defaults.package_priority,
        "package_priority",
        df,
    )?;
    let package_homepage = pick(
        m.homepage.clone(),
        defaults.package_homepage,
        "package_homepage",
        df,
    )?;
    let package_installed_size =
        pick_optional(m.installed_size.clone(), defaults.package_installed_size);
    let package_source = pick_optional(m.source.clone(), defaults.package_source);
    let architecture = pick(
        m.architecture.clone(),
        defaults.architecture,
        "architecture",
        df,
    )?;
    let license = pick(m.license.clone(), defaults.license, "license", df)?;
    let githash = pick(m.githash.clone(), defaults.githash, "githash", df)?;
    let buildurl = pick(m.buildurl.clone(), defaults.buildurl, "buildurl", df)?;
    let package_description = pick(
        m.description.clone(),
        defaults.package_description,
        "package_description",
        df,
    )?;

    Ok(Input {
        build_dir: args.build_dir.clone(),
        output_dir: args.output_dir.clone(),
        clean: args.clean,
        package_name: args.package_name.clone(),
        version: args.version.clone(),
        vendor,
        origin,
        label,
        package_authors,
        package_maintainer,
        package_description,
        package_section,
        package_priority,
        package_homepage,
        package_installed_size,
        package_source,
        architecture,
        suite: args.suite.clone(),
        codename: args.codename.clone(),
        depends,
        suggested_depends,
        recommended_depends,
        pre_depends,
        conflicts,
        replaces,
        breaks,
        provides,
        license,
        githash,
        buildurl,
        debug: args.debug,
    })
}

fn add_installed_size(
    dir: &Path,
    is_root: bool,
    seen: &mut HashSet<(u64, u64)>,
    total: &mut u64,
) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        if is_root && entry.file_name() == "DEBIAN" {
            continue;
        }
        let meta = entry.path().symlink_metadata()?;
        if meta.is_dir() {
            *total += 1;
            add_installed_size(&entry.path(), false, seen, total)?;
        } else if !meta.is_file() && !meta.is_symlink() {
            *total += 1;
        } else if meta.nlink() == 1 || seen.insert((meta.dev(), meta.ino())) {
            // A hard link adds no content, so count each inode once.
            *total += meta.len().div_ceil(1024);
        }
    }
    Ok(())
}

/// Compute `Installed-Size` in KiB, with the rule of `dpkg-gencontrol` 1.23: a
/// file or symlink adds its size rounded up to 1 KiB, and any other object adds
/// 1 KiB, the root directory included.
///
/// There is one exception: `dpkg-gencontrol` counts `DEBIAN`, and this does
/// not, because the field describes what is installed.
fn installed_size_kib(build_dir: &Path) -> Result<u64> {
    let mut total = 1;
    add_installed_size(build_dir, true, &mut HashSet::new(), &mut total)
        .with_context(|| format!("Computing Installed-Size of {}", build_dir.display()))?;
    Ok(total)
}

fn to_control(input: &Input) -> DebianControl {
    DebianControl {
        package_name: input.package_name.clone(),
        version: input.version.clone(),
        vendor: input.vendor.clone(),
        origin: input.origin.clone(),
        label: input.label.clone(),
        package_authors: input.package_authors.clone(),
        package_maintainer: input.package_maintainer.clone(),
        package_description: input.package_description.clone(),
        package_section: input.package_section.clone(),
        package_priority: input.package_priority.clone(),
        package_homepage: input.package_homepage.clone(),
        package_installed_size: input.package_installed_size.clone(),
        package_source: input.package_source.clone(),
        architecture: input.architecture.clone(),
        suite: input.suite.clone(),
        codename: input.codename.clone(),
        depends: input.depends.clone(),
        suggested_depends: input.suggested_depends.clone(),
        recommended_depends: input.recommended_depends.clone(),
        pre_depends: input.pre_depends.clone(),
        conflicts: input.conflicts.clone(),
        replaces: input.replaces.clone(),
        breaks: input.breaks.clone(),
        provides: input.provides.clone(),
        license: input.license.clone(),
        githash: input.githash.clone(),
        buildurl: input.buildurl.clone(),
    }
}

pub fn build_debian_package(input: &Input) -> Result<()> {
    use std::process::Command;

    let entries: Vec<_> = std::fs::read_dir(&input.build_dir)
        .map_err(|e| anyhow!("Failed to read build dir {}: {}", input.build_dir, e))?
        .collect();
    if entries.is_empty() {
        anyhow::bail!("Debian build directory is empty {}", input.build_dir);
    }

    let mut control = to_control(input);
    if control.package_installed_size.trim().is_empty() {
        control.package_installed_size =
            installed_size_kib(Path::new(&input.build_dir))?.to_string();
    }
    let control_text = format_control_file(&control)?;

    let control_dir = Path::new(&input.build_dir).join("DEBIAN");
    std::fs::create_dir_all(&control_dir)?;
    let control_file = control_dir.join("control");
    std::fs::write(&control_file, control_text.as_bytes())?;

    log::info!("Building debian package...");
    std::fs::create_dir_all(&input.output_dir)?;

    let deb_name = DebName {
        package: &input.package_name,
        version: &input.version,
        architecture: Some(&input.architecture),
    }
    .to_string();
    let deb_output = format!("{}/{}", input.output_dir, deb_name);

    if input.debug {
        log::info!(
            "Executing: fakeroot dpkg-deb -Zgzip --build {} {}",
            input.build_dir,
            deb_output
        );
    }

    let output = Command::new("fakeroot")
        .args(["dpkg-deb", "-Zgzip", "--build"])
        .arg(&input.build_dir)
        .arg(&deb_output)
        .output()
        .map_err(|e| anyhow!("Failed to spawn fakeroot/dpkg-deb: {}", e))?;

    if !output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        log::error!(
            "Failed to build package {}. Stdout: {} , Stderr: {}",
            deb_name,
            stdout,
            stderr
        );
        anyhow::bail!("Failed to build debian package {}", deb_name);
    }

    log::info!("Package {} built at {}", deb_name, input.output_dir);

    if input.clean {
        log::info!("Cleaning up...");
        std::fs::remove_dir_all(&input.build_dir)?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    fn write(path: &Path, size: usize) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, vec![0u8; size]).unwrap();
    }

    fn size(root: &Path) -> u64 {
        installed_size_kib(root).unwrap()
    }

    #[test]
    fn a_file_size_rounds_up_to_one_kib() {
        let cases = [(0, 0), (1, 1), (1024, 1), (1025, 2)];
        for (bytes, kib) in cases {
            let dir = tempfile::tempdir().unwrap();
            write(&dir.path().join("x"), bytes);
            assert_eq!(size(dir.path()), 1 + kib, "{} bytes", bytes);
        }
    }

    #[test]
    fn each_directory_adds_one_kib() {
        let dir = tempfile::tempdir().unwrap();
        write(&dir.path().join("a/b/x"), 2048);
        assert_eq!(size(dir.path()), 1 + 2 + 2);
    }

    #[test]
    fn debian_directory_does_not_count() {
        let dir = tempfile::tempdir().unwrap();
        write(&dir.path().join("DEBIAN/control"), 4096);
        write(&dir.path().join("x"), 1024);
        assert_eq!(size(dir.path()), 1 + 1);
    }

    #[test]
    fn only_the_top_level_debian_directory_is_skipped() {
        let dir = tempfile::tempdir().unwrap();
        write(&dir.path().join("usr/DEBIAN/x"), 1024);
        assert_eq!(size(dir.path()), 1 + 2 + 1);
    }

    #[test]
    fn a_hard_link_counts_once() {
        let dir = tempfile::tempdir().unwrap();
        write(&dir.path().join("x"), 2048);
        std::fs::hard_link(dir.path().join("x"), dir.path().join("y")).unwrap();
        assert_eq!(size(dir.path()), 1 + 2);
    }

    #[test]
    fn a_symlink_counts_its_own_size_and_not_its_target() {
        let dir = tempfile::tempdir().unwrap();
        write(&dir.path().join("x"), 4096);
        symlink("x", dir.path().join("y")).unwrap();
        assert_eq!(size(dir.path()), 1 + 4 + 1);
    }
}
