//! The Debian package file name: `<package>_<version>_<architecture>.deb`.
//!
//! `_` is a safe separator, because Debian package names and versions cannot
//! contain it. The architecture is optional when a name is read, because older
//! packages from `build` do not have it.

use anyhow::{anyhow, Result};
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DebName<'a> {
    pub package: &'a str,
    pub version: &'a str,
    pub architecture: Option<&'a str>,
}

impl<'a> DebName<'a> {
    /// Read a file name, without its directory.
    pub fn parse(file_name: &'a str) -> Result<Self> {
        let stem = file_name.strip_suffix(".deb").unwrap_or(file_name);
        let mut parts = stem.splitn(3, '_');
        match (parts.next(), parts.next(), parts.next()) {
            (Some(package), Some(version), architecture) => Ok(Self {
                package,
                version,
                architecture,
            }),
            _ => Err(anyhow!(
                "Filename {} is not in <package>_<version>[_<architecture>].deb form",
                file_name
            )),
        }
    }
}

impl fmt::Display for DebName<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}_{}", self.package, self.version)?;
        if let Some(architecture) = self.architecture {
            write!(f, "_{}", architecture)?;
        }
        write!(f, ".deb")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a_name_with_an_architecture() {
        let name = DebName::parse("mina-logproc_3.1.0-b4ed493_amd64.deb").unwrap();
        assert_eq!(name.package, "mina-logproc");
        assert_eq!(name.version, "3.1.0-b4ed493");
        assert_eq!(name.architecture, Some("amd64"));
    }

    #[test]
    fn reads_a_name_without_an_architecture() {
        let name = DebName::parse("mina-logproc_3.1.0.deb").unwrap();
        assert_eq!(name.version, "3.1.0");
        assert_eq!(name.architecture, None);
    }

    #[test]
    fn rejects_a_name_without_a_version() {
        assert!(DebName::parse("mina-logproc.deb").is_err());
    }

    #[test]
    fn a_name_survives_a_round_trip() {
        for text in ["a_1.0_amd64.deb", "a_1.0_all.deb", "a_1.0.deb"] {
            assert_eq!(DebName::parse(text).unwrap().to_string(), text);
        }
    }
}
