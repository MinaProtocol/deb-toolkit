//! Read a `.deb` into memory: its control data and a list of what it installs.
//!
//! Data members are streamed and hashed, never extracted, so a large package
//! needs no disk space and little memory.

use anyhow::{bail, Context, Result};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::fs::File;
use std::io::{self, Read};
use std::path::Path;

use crate::misc::check_file_exists;
use crate::session::Compression;

/// `ar` members that hold a signature.
const SIGNATURE_MEMBERS: &[&str] = &["_gpgorigin", "_gpgbuilder"];

/// One entry of `data.tar`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Entry {
    pub(super) kind: &'static str,
    pub(super) mode: u32,
    pub(super) uid: u64,
    pub(super) gid: u64,
    pub(super) user: String,
    pub(super) group: String,
    pub(super) mtime: u64,
    pub(super) size: u64,
    pub(super) sha256: Option<String>,
    pub(super) link: Option<String>,
}

impl Entry {
    pub(super) fn owner(&self) -> String {
        if self.user.is_empty() && self.group.is_empty() {
            format!("{}:{}", self.uid, self.gid)
        } else {
            format!("{}:{} ({}:{})", self.user, self.group, self.uid, self.gid)
        }
    }

    /// dpkg prefers an owner name and uses the number when there is no name,
    /// so a missing name is not a difference.
    pub(super) fn same_owner(&self, other: &Entry) -> bool {
        fn same_name(a: &str, b: &str) -> bool {
            a.is_empty() || b.is_empty() || a == b
        }
        self.uid == other.uid
            && self.gid == other.gid
            && same_name(&self.user, &other.user)
            && same_name(&self.group, &other.group)
    }

    pub(super) fn describe(&self) -> String {
        let mut text = format!("{} {:04o} {}", self.kind, self.mode, self.owner());
        if let Some(link) = &self.link {
            let _ = write!(text, " -> {}", link);
        } else if self.kind == "file" {
            let _ = write!(text, " {} bytes", self.size);
        }
        text
    }
}

/// A control field, with its name as the package spells it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Field {
    pub(super) name: String,
    pub(super) value: String,
}

#[derive(Debug, Default)]
pub(super) struct Package {
    /// Field names are case-insensitive, so the key is lower-case.
    pub(super) fields: BTreeMap<String, Field>,
    /// Control archive members, except `control`.
    pub(super) control_files: BTreeMap<String, Vec<u8>>,
    pub(super) entries: BTreeMap<String, Entry>,
    pub(super) control_compression: String,
    pub(super) data_compression: String,
    pub(super) signatures: BTreeSet<String>,
    pub(super) format_version: String,
}

/// Parse a binary package control file into its fields.
fn parse_control(text: &str) -> BTreeMap<String, Field> {
    let mut fields: BTreeMap<String, Field> = BTreeMap::new();
    let mut current: Option<String> = None;
    for line in text.lines() {
        if line.starts_with([' ', '\t']) {
            if let Some(field) = current.as_ref().and_then(|key| fields.get_mut(key)) {
                field.value.push('\n');
                field.value.push_str(line.trim_end());
            }
        } else if let Some((name, value)) = line.split_once(':') {
            let key = name.trim().to_ascii_lowercase();
            fields.insert(
                key.clone(),
                Field {
                    name: name.trim().to_string(),
                    value: value.trim().to_string(),
                },
            );
            current = Some(key);
        }
    }
    fields
}

/// `./usr/bin/` in a tar header becomes `/usr/bin`, as dpkg shows it.
fn normalize_path(path: &Path) -> String {
    let text = path.to_string_lossy();
    let text = text.strip_prefix('.').unwrap_or(&text);
    format!("/{}", text.trim_matches('/'))
}

fn kind_name(kind: tar::EntryType) -> &'static str {
    match kind {
        tar::EntryType::Regular | tar::EntryType::Continuous | tar::EntryType::GNUSparse => "file",
        tar::EntryType::Directory => "directory",
        tar::EntryType::Symlink => "symlink",
        tar::EntryType::Link => "hardlink",
        tar::EntryType::Char => "character device",
        tar::EntryType::Block => "block device",
        tar::EntryType::Fifo => "fifo",
        _ => "other",
    }
}

fn read_control_tar(input: impl Read, package: &mut Package) -> Result<()> {
    let mut archive = tar::Archive::new(input);
    for entry in archive.entries()? {
        let mut entry = entry?;
        if !entry.header().entry_type().is_file() {
            continue;
        }
        let name = normalize_path(&entry.path()?);
        let name = name.trim_start_matches('/').to_string();
        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes)?;
        if name == "control" {
            let text = String::from_utf8(bytes).context("The control file is not UTF-8")?;
            package.fields = parse_control(&text);
        } else {
            package.control_files.insert(name, bytes);
        }
    }
    Ok(())
}

fn read_data_tar(input: impl Read, package: &mut Package) -> Result<()> {
    let mut archive = tar::Archive::new(input);
    for entry in archive.entries()? {
        let mut entry = entry?;
        let header = entry.header();
        let kind = kind_name(header.entry_type());
        let mode = header.mode()? & 0o7777;
        let (uid, gid, mtime) = (header.uid()?, header.gid()?, header.mtime()?);
        let user = header.username().ok().flatten().unwrap_or("").to_string();
        let group = header.groupname().ok().flatten().unwrap_or("").to_string();
        let path = normalize_path(&entry.path()?);
        // dpkg-deb writes a `./` entry and `session save` does not. It is the
        // root of the target system, so no package installs it.
        if path == "/" {
            continue;
        }
        let link = entry.link_name()?.map(|target| match kind {
            // A hard link names another member of the archive.
            "hardlink" => normalize_path(&target),
            _ => target.to_string_lossy().into_owned(),
        });

        let (size, sha256) = if kind == "file" {
            let mut hasher = Sha256::new();
            let size = io::copy(&mut entry, &mut hasher)?;
            (size, Some(hex(&hasher.finalize())))
        } else {
            (0, None)
        };

        package.entries.insert(
            path,
            Entry {
                kind,
                mode,
                uid,
                gid,
                user,
                group,
                mtime,
                size,
                sha256,
                link,
            },
        );
    }
    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().fold(String::new(), |mut text, byte| {
        let _ = write!(text, "{:02x}", byte);
        text
    })
}

fn compression_of(member: &str) -> Result<(Compression, String)> {
    let compression = Compression::from_filename(member)?;
    Ok((compression, compression.as_keyword().to_string()))
}

pub(super) fn read_package(path: &str) -> Result<Package> {
    check_file_exists(path)?;
    let file = File::open(path).with_context(|| format!("Opening {}", path))?;
    let mut archive = ar::Archive::new(file);
    let mut package = Package::default();
    let (mut has_control, mut has_data) = (false, false);

    while let Some(member) = archive.next_entry() {
        let mut member = member.with_context(|| format!("{} is not a Debian package", path))?;
        let name = String::from_utf8_lossy(member.header().identifier())
            .trim_end_matches('/')
            .to_string();

        if name == "debian-binary" {
            member.read_to_string(&mut package.format_version)?;
            package.format_version = package.format_version.trim().to_string();
        } else if name.starts_with("control.tar") {
            let (compression, keyword) = compression_of(&name)?;
            package.control_compression = keyword;
            read_control_tar(compression.reader(member)?, &mut package)
                .with_context(|| format!("Reading {} from {}", name, path))?;
            has_control = true;
        } else if name.starts_with("data.tar") {
            let (compression, keyword) = compression_of(&name)?;
            package.data_compression = keyword;
            read_data_tar(compression.reader(member)?, &mut package)
                .with_context(|| format!("Reading {} from {}", name, path))?;
            has_data = true;
        } else if SIGNATURE_MEMBERS.contains(&name.as_str()) {
            package.signatures.insert(name);
        }
    }

    if !has_control || !has_data || package.fields.is_empty() {
        bail!(
            "{} is not a Debian package: control or data is missing",
            path
        );
    }
    Ok(package)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn control_parser_keeps_continuation_lines_and_folds_case() {
        let fields = parse_control("Package: x\nDESCRIPTION: short\n long line\n .\n more  \n");
        assert_eq!(fields["package"].value, "x");
        assert_eq!(fields["description"].name, "DESCRIPTION");
        assert_eq!(fields["description"].value, "short\n long line\n .\n more");
    }

    #[test]
    fn tar_paths_match_installed_paths() {
        assert_eq!(normalize_path(&PathBuf::from("./")), "/");
        assert_eq!(normalize_path(&PathBuf::from("./usr/bin/")), "/usr/bin");
        assert_eq!(normalize_path(&PathBuf::from("usr/bin/x")), "/usr/bin/x");
        assert_eq!(normalize_path(&PathBuf::from("./.hidden")), "/.hidden");
    }
}
