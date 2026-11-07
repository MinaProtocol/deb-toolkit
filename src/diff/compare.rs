//! Compare two packages, and hold the result.

use anyhow::{Context, Result};
use glob::Pattern;
use regex::Regex;
use serde::Serialize;
use similar::TextDiff;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::LazyLock;

use super::package::{read_package, Entry, Field, Package};
use crate::cli::DiffArgs;

/// Control fields that hold a comma-separated list of package relations. Their
/// order has no meaning, so they are compared as sets.
const RELATION_FIELDS: &[&str] = &[
    "depends",
    "pre-depends",
    "recommends",
    "suggests",
    "enhances",
    "breaks",
    "conflicts",
    "replaces",
    "provides",
];

static VERSION_CONSTRAINT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\(\s*(<<|<=|=|>=|>>)\s*([^\s)]+)\s*\)").unwrap());

#[derive(Debug, Default, Clone)]
pub struct DiffOptions {
    /// Lower-case control field names to skip.
    pub ignore_fields: BTreeSet<String>,
    pub ignore_paths: Vec<Pattern>,
    pub compare_mtime: bool,
}

impl DiffOptions {
    pub fn from_args(args: &DiffArgs) -> Result<Self> {
        let ignore_paths = args
            .ignore_paths
            .iter()
            .map(|p| Pattern::new(p).with_context(|| format!("Invalid --ignore-path glob: {}", p)))
            .collect::<Result<_>>()?;
        Ok(Self {
            ignore_fields: args
                .ignore_fields
                .iter()
                .map(|f| f.to_ascii_lowercase())
                .collect(),
            ignore_paths,
            compare_mtime: args.compare_mtime,
        })
    }

    fn skips_path(&self, path: &str) -> bool {
        self.ignore_paths.iter().any(|p| p.matches(path))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ChangeKind {
    Added,
    Removed,
    Changed,
}

/// A container difference. It does not change what a package installs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Note {
    pub item: String,
    pub old: String,
    pub new: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FieldChange {
    pub name: String,
    pub kind: ChangeKind,
    pub old: Option<String>,
    pub new: Option<String>,
    /// For a relation field: the relations only in the new package.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub added: Vec<String>,
    /// For a relation field: the relations only in the old package.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub removed: Vec<String>,
}

/// A change to a member of the control archive, for example `postinst`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ControlFileChange {
    pub name: String,
    pub kind: ChangeKind,
    /// Unified diff, when both sides are text.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diff: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AttributeChange {
    pub attribute: String,
    pub old: String,
    pub new: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FileChange {
    pub path: String,
    pub kind: ChangeKind,
    /// The added or removed entry, as text. Empty for a changed entry.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entry: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub changes: Vec<AttributeChange>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DebDiff {
    pub old: String,
    pub new: String,
    pub identical: bool,
    pub notes: Vec<Note>,
    pub fields: Vec<FieldChange>,
    pub control_files: Vec<ControlFileChange>,
    pub files: Vec<FileChange>,
    /// Paths compared after `--ignore-path`, in either package.
    pub paths_compared: usize,
}

/// Split a relation field into a set, with one spelling for each relation.
fn parse_relations(value: &str) -> BTreeSet<String> {
    value
        .split(',')
        .map(|item| item.split_whitespace().collect::<Vec<_>>().join(" "))
        .map(|item| {
            VERSION_CONSTRAINT
                .replace_all(&item, "($1 $2)")
                .into_owned()
        })
        .filter(|item| !item.is_empty())
        .collect()
}

fn compare_notes(old: &Package, new: &Package) -> Vec<Note> {
    fn signatures(package: &Package) -> String {
        if package.signatures.is_empty() {
            return "unsigned".to_string();
        }
        let names: Vec<&str> = package.signatures.iter().map(String::as_str).collect();
        names.join(", ")
    }

    [
        (
            "format",
            old.format_version.clone(),
            new.format_version.clone(),
        ),
        (
            "control compression",
            old.control_compression.clone(),
            new.control_compression.clone(),
        ),
        (
            "data compression",
            old.data_compression.clone(),
            new.data_compression.clone(),
        ),
        ("signature", signatures(old), signatures(new)),
    ]
    .into_iter()
    .filter(|(_, a, b)| a != b)
    .map(|(item, old, new)| Note {
        item: item.to_string(),
        old,
        new,
    })
    .collect()
}

fn compare_field(key: &str, old: &Field, new: &Field) -> Option<FieldChange> {
    let (mut added, mut removed) = (Vec::new(), Vec::new());
    if RELATION_FIELDS.contains(&key) {
        let (a, b) = (parse_relations(&old.value), parse_relations(&new.value));
        if a == b {
            return None;
        }
        added = b.difference(&a).cloned().collect();
        removed = a.difference(&b).cloned().collect();
    } else if old.value == new.value {
        return None;
    }
    Some(FieldChange {
        name: new.name.clone(),
        kind: ChangeKind::Changed,
        old: Some(old.value.clone()),
        new: Some(new.value.clone()),
        added,
        removed,
    })
}

fn compare_fields(old: &Package, new: &Package, options: &DiffOptions) -> Vec<FieldChange> {
    let keys: BTreeSet<&String> = old.fields.keys().chain(new.fields.keys()).collect();
    keys.into_iter()
        .filter(|key| !options.ignore_fields.contains(*key))
        .filter_map(|key| match (old.fields.get(key), new.fields.get(key)) {
            (Some(a), Some(b)) => compare_field(key, a, b),
            (Some(a), None) => Some(FieldChange {
                name: a.name.clone(),
                kind: ChangeKind::Removed,
                old: Some(a.value.clone()),
                new: None,
                added: Vec::new(),
                removed: Vec::new(),
            }),
            (None, Some(b)) => Some(FieldChange {
                name: b.name.clone(),
                kind: ChangeKind::Added,
                old: None,
                new: Some(b.value.clone()),
                added: Vec::new(),
                removed: Vec::new(),
            }),
            (None, None) => None,
        })
        .collect()
}

fn text_diff(name: &str, old: &[u8], new: &[u8]) -> Option<String> {
    let (old, new) = (
        std::str::from_utf8(old).ok()?,
        std::str::from_utf8(new).ok()?,
    );
    let diff = TextDiff::from_lines(old, new)
        .unified_diff()
        .context_radius(2)
        .header(&format!("old/{}", name), &format!("new/{}", name))
        .to_string();
    Some(diff)
}

fn compare_control_files(old: &Package, new: &Package) -> Vec<ControlFileChange> {
    let names: BTreeSet<&String> = old
        .control_files
        .keys()
        .chain(new.control_files.keys())
        .collect();
    names
        .into_iter()
        .filter_map(|name| {
            let (a, b) = (old.control_files.get(name), new.control_files.get(name));
            let (kind, diff) = match (a, b) {
                (Some(a), Some(b)) if a == b => return None,
                (Some(a), Some(b)) => (ChangeKind::Changed, text_diff(name, a, b)),
                (Some(_), None) => (ChangeKind::Removed, None),
                _ => (ChangeKind::Added, None),
            };
            Some(ControlFileChange {
                name: name.clone(),
                kind,
                diff,
            })
        })
        .collect()
}

fn compare_entry(old: &Entry, new: &Entry, options: &DiffOptions) -> Vec<AttributeChange> {
    let change = |attribute: &str, old: String, new: String| AttributeChange {
        attribute: attribute.to_string(),
        old,
        new,
    };

    // Other attributes have no common meaning across two types.
    if old.kind != new.kind {
        return vec![change("type", old.kind.to_string(), new.kind.to_string())];
    }

    let mut changes = Vec::new();
    if old.sha256 != new.sha256 {
        changes.push(change(
            "content",
            format!(
                "{} bytes, sha256 {}",
                old.size,
                old.sha256.as_deref().unwrap_or("-")
            ),
            format!(
                "{} bytes, sha256 {}",
                new.size,
                new.sha256.as_deref().unwrap_or("-")
            ),
        ));
    }
    if old.link != new.link {
        changes.push(change(
            "link",
            old.link.clone().unwrap_or_default(),
            new.link.clone().unwrap_or_default(),
        ));
    }
    if old.mode != new.mode {
        changes.push(change(
            "mode",
            format!("{:04o}", old.mode),
            format!("{:04o}", new.mode),
        ));
    }
    if !old.same_owner(new) {
        changes.push(change("owner", old.owner(), new.owner()));
    }
    if options.compare_mtime && old.mtime != new.mtime {
        changes.push(change(
            "mtime",
            old.mtime.to_string(),
            new.mtime.to_string(),
        ));
    }
    changes
}

fn compare_entries(
    old: &BTreeMap<String, Entry>,
    new: &BTreeMap<String, Entry>,
    options: &DiffOptions,
) -> (Vec<FileChange>, usize) {
    let paths: BTreeSet<&String> = old.keys().chain(new.keys()).collect();
    let mut compared = 0;
    let mut changes = Vec::new();

    for path in paths {
        if options.skips_path(path) {
            continue;
        }
        compared += 1;
        let (kind, entry, attributes) = match (old.get(path), new.get(path)) {
            (Some(a), Some(b)) => {
                let attributes = compare_entry(a, b, options);
                if attributes.is_empty() {
                    continue;
                }
                (ChangeKind::Changed, None, attributes)
            }
            (Some(a), None) => (ChangeKind::Removed, Some(a.describe()), Vec::new()),
            (None, Some(b)) => (ChangeKind::Added, Some(b.describe()), Vec::new()),
            (None, None) => continue,
        };
        changes.push(FileChange {
            path: path.clone(),
            kind,
            entry,
            changes: attributes,
        });
    }
    (changes, compared)
}

/// Compare the packages at `old_path` and `new_path`.
pub fn compare(old_path: &str, new_path: &str, options: &DiffOptions) -> Result<DebDiff> {
    let old = read_package(old_path)?;
    let new = read_package(new_path)?;

    let fields = compare_fields(&old, &new, options);
    let control_files = compare_control_files(&old, &new);
    let (files, paths_compared) = compare_entries(&old.entries, &new.entries, options);

    Ok(DebDiff {
        old: old_path.to_string(),
        new: new_path.to_string(),
        identical: fields.is_empty() && control_files.is_empty() && files.is_empty(),
        notes: compare_notes(&old, &new),
        fields,
        control_files,
        files,
        paths_compared,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(content: &str) -> Entry {
        Entry {
            kind: "file",
            mode: 0o644,
            uid: 0,
            gid: 0,
            user: "root".into(),
            group: "root".into(),
            mtime: 100,
            size: content.len() as u64,
            sha256: Some(content.to_string()),
            link: None,
        }
    }

    fn map(entries: &[(&str, Entry)]) -> BTreeMap<String, Entry> {
        entries
            .iter()
            .map(|(path, entry)| (path.to_string(), entry.clone()))
            .collect()
    }

    fn attributes(old: Entry, new: Entry, options: &DiffOptions) -> Vec<String> {
        compare_entry(&old, &new, options)
            .into_iter()
            .map(|change| change.attribute)
            .collect()
    }

    #[test]
    fn relation_spelling_and_order_do_not_matter() {
        assert_eq!(
            parse_relations("b  (>=1.0), a (= 2)"),
            parse_relations("a (=  2) ,b (>= 1.0)")
        );
    }

    #[test]
    fn relation_alternatives_stay_together() {
        let relations = parse_relations("a | b, c");
        assert!(relations.contains("a | b"));
        assert_eq!(relations.len(), 2);
    }

    #[test]
    fn empty_relation_field_is_an_empty_set() {
        assert!(parse_relations(" , ").is_empty());
    }

    #[test]
    fn relation_change_lists_each_side() {
        let field = |value: &str| Field {
            name: "Depends".into(),
            value: value.into(),
        };
        let change = compare_field("depends", &field("a, b"), &field("b, c")).unwrap();
        assert_eq!(change.added, vec!["c"]);
        assert_eq!(change.removed, vec!["a"]);
    }

    #[test]
    fn other_fields_compare_as_text() {
        let field = |value: &str| Field {
            name: "Section".into(),
            value: value.into(),
        };
        assert!(compare_field("section", &field("a, b"), &field("b, a")).is_some());
    }

    #[test]
    fn mtime_counts_only_on_request() {
        let mut new = file("x");
        new.mtime = 200;
        assert!(attributes(file("x"), new.clone(), &DiffOptions::default()).is_empty());
        let options = DiffOptions {
            compare_mtime: true,
            ..Default::default()
        };
        assert_eq!(attributes(file("x"), new, &options), vec!["mtime"]);
    }

    #[test]
    fn a_missing_owner_name_is_not_a_difference() {
        let mut new = file("x");
        new.user = String::new();
        new.group = String::new();
        assert!(attributes(file("x"), new, &DiffOptions::default()).is_empty());
    }

    #[test]
    fn a_different_owner_name_or_number_is_a_difference() {
        let mut named = file("x");
        named.user = "mina".into();
        assert_eq!(
            attributes(file("x"), named, &DiffOptions::default()),
            vec!["owner"]
        );
        let mut numbered = file("x");
        numbered.gid = 1000;
        assert_eq!(
            attributes(file("x"), numbered, &DiffOptions::default()),
            vec!["owner"]
        );
    }

    #[test]
    fn mode_content_and_link_are_each_reported() {
        let mut new = file("y");
        new.mode = 0o755;
        assert_eq!(
            attributes(file("x"), new, &DiffOptions::default()),
            vec!["content", "mode"]
        );

        let mut old = file("x");
        old.kind = "symlink";
        old.sha256 = None;
        old.link = Some("a".into());
        let mut new = old.clone();
        new.link = Some("b".into());
        assert_eq!(attributes(old, new, &DiffOptions::default()), vec!["link"]);
    }

    #[test]
    fn a_type_change_hides_other_attributes() {
        let mut new = file("x");
        new.kind = "symlink";
        new.sha256 = None;
        new.mode = 0o777;
        assert_eq!(
            attributes(file("x"), new, &DiffOptions::default()),
            vec!["type"]
        );
    }

    #[test]
    fn ignored_paths_are_not_compared_or_counted() {
        let old = map(&[("/a/x", file("1")), ("/b", file("1"))]);
        let new = map(&[("/a/x", file("2")), ("/b", file("1"))]);
        let options = DiffOptions {
            ignore_paths: vec![Pattern::new("/a/*").unwrap()],
            ..Default::default()
        };
        let (changes, compared) = compare_entries(&old, &new, &options);
        assert!(changes.is_empty());
        assert_eq!(compared, 1);
    }

    #[test]
    fn added_and_removed_paths_are_found() {
        let old = map(&[("/gone", file("1"))]);
        let new = map(&[("/new", file("1"))]);
        let (changes, compared) = compare_entries(&old, &new, &DiffOptions::default());
        assert_eq!(compared, 2);
        assert_eq!(changes[0].path, "/gone");
        assert_eq!(changes[0].kind, ChangeKind::Removed);
        assert_eq!(changes[1].path, "/new");
        assert_eq!(changes[1].kind, ChangeKind::Added);
    }

    #[test]
    fn binary_control_files_have_no_text_diff() {
        assert!(text_diff("x", b"\xff\xfe", b"a").is_none());
        assert!(text_diff("x", b"a\n", b"b\n").unwrap().contains("+b"));
    }
}
