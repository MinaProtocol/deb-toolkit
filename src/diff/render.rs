//! Render a comparison for a terminal.

use std::fmt::Write as _;

use super::compare::{ChangeKind, DebDiff, FieldChange};

fn sign(kind: ChangeKind) -> char {
    match kind {
        ChangeKind::Added => '+',
        ChangeKind::Removed => '-',
        ChangeKind::Changed => '~',
    }
}

fn render_field(out: &mut String, field: &FieldChange) {
    let sign = sign(field.kind);
    let value = |v: &Option<String>| v.as_deref().unwrap_or("").replace('\n', "\n      ");
    match field.kind {
        ChangeKind::Added => {
            let _ = writeln!(out, "  {} {}: {}", sign, field.name, value(&field.new));
        }
        ChangeKind::Removed => {
            let _ = writeln!(out, "  {} {}: {}", sign, field.name, value(&field.old));
        }
        ChangeKind::Changed if !field.added.is_empty() || !field.removed.is_empty() => {
            let _ = writeln!(out, "  {} {}:", sign, field.name);
            for item in &field.removed {
                let _ = writeln!(out, "      - {}", item);
            }
            for item in &field.added {
                let _ = writeln!(out, "      + {}", item);
            }
        }
        ChangeKind::Changed => {
            let _ = writeln!(out, "  {} {}:", sign, field.name);
            let _ = writeln!(out, "      - {}", value(&field.old));
            let _ = writeln!(out, "      + {}", value(&field.new));
        }
    }
}

/// Render the report for a terminal.
pub fn render(diff: &DebDiff) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "--- {}\n+++ {}", diff.old, diff.new);

    if !diff.notes.is_empty() {
        let _ = writeln!(out, "\nNotes (these do not count as differences)");
        for note in &diff.notes {
            let _ = writeln!(out, "  {}: {} -> {}", note.item, note.old, note.new);
        }
    }
    if !diff.fields.is_empty() {
        let _ = writeln!(out, "\nControl fields ({})", diff.fields.len());
        for field in &diff.fields {
            render_field(&mut out, field);
        }
    }
    if !diff.control_files.is_empty() {
        let _ = writeln!(out, "\nControl files ({})", diff.control_files.len());
        for file in &diff.control_files {
            let _ = writeln!(out, "  {} {}", sign(file.kind), file.name);
            if let Some(text) = &file.diff {
                for line in text.lines() {
                    let _ = writeln!(out, "      {}", line);
                }
            } else if file.kind == ChangeKind::Changed {
                let _ = writeln!(out, "      binary content differs");
            }
        }
    }
    if !diff.files.is_empty() {
        let _ = writeln!(out, "\nFiles ({})", diff.files.len());
        for file in &diff.files {
            match &file.entry {
                Some(entry) => {
                    let _ = writeln!(out, "  {} {}  {}", sign(file.kind), file.path, entry);
                }
                None => {
                    let _ = writeln!(out, "  {} {}", sign(file.kind), file.path);
                }
            }
            for change in &file.changes {
                let _ = writeln!(
                    out,
                    "      {}: {} -> {}",
                    change.attribute, change.old, change.new
                );
            }
        }
    }

    let differences = diff.fields.len() + diff.control_files.len() + diff.files.len();
    if diff.identical {
        let _ = writeln!(
            out,
            "\nResult: packages match ({} paths compared)",
            diff.paths_compared
        );
    } else {
        let _ = writeln!(
            out,
            "\nResult: {} differences ({} paths compared)",
            differences, diff.paths_compared
        );
    }
    out
}
