//! Compare two Debian packages by what they install.
//!
//! The main use is the migration from the bash package builder to
//! `deb-toolkit build`: build one package both ways, and prove that the
//! result is the same. `cmp` cannot do this, because two correct packages can
//! differ in timestamps, tar order and compression.
//!
//! The report has two levels:
//!
//!   * **Differences** change what a user gets: control fields, maintainer
//!     scripts, and installed files with their type, mode and owner. They make
//!     the command exit with 1.
//!   * **Notes** change only the container: compression and signatures. They
//!     are shown, but they do not fail the comparison.
//!
//! Modification times differ between any two builds, so they are compared
//! only on request.
//!
//! The work has three steps: [`package`] reads a `.deb`, [`compare`] compares
//! two packages, and [`render`] prints the result.

mod compare;
mod package;
mod render;

use anyhow::{anyhow, Result};
use std::io::{self, Write as _};

use crate::cli::DiffArgs;

pub use compare::{
    compare, AttributeChange, ChangeKind, ControlFileChange, DebDiff, DiffOptions, FieldChange,
    FileChange, Note,
};
pub use render::render;

/// Run the `diff` command. Returns true when the packages match.
pub fn run(args: &DiffArgs) -> Result<bool> {
    let options = DiffOptions::from_args(args)?;
    let diff = compare(&args.old, &args.new, &options)?;
    let report = if args.json {
        let mut text = serde_json::to_string_pretty(&diff)?;
        text.push('\n');
        text
    } else {
        render(&diff)
    };

    // A closed pipe, for example from `| head`, is not an error here.
    match io::stdout().write_all(report.as_bytes()) {
        Err(err) if err.kind() != io::ErrorKind::BrokenPipe => {
            return Err(anyhow!("Writing the report: {}", err));
        }
        _ => {}
    }
    Ok(diff.identical)
}
