use anyhow::{Context, Result};
use minijinja::{context, Environment};
use serde::Serialize;

const POLICY_FILE_TEMPLATE: &str = r#"<?xml version="1.0"?>
<!DOCTYPE Policy SYSTEM "https://www.debian.org/debsig/1.0/policy.dtd">
<Policy xmlns="https://www.debian.org/debsig/1.0/">

  <!-- Here name and description can be anything. -->
  <Origin Name="Verification" id="{{ key_id }}" Description="{{ description }}" />

  <Selection>
    <Required Type="origin" File="{{ key_filename }}" id="{{ key_id }}"/>
  </Selection>

  <Verification MinOptional="0">
    <Required Type="origin" File="{{ key_filename }}" id="{{ key_id }}"/>
  </Verification>

</Policy>
"#;

const DEBIAN_CONTROL_FILE_TEMPLATE: &str = r#"
{%- autoescape false -%}
{% for property in properties %}{{ property.name }}: {{ property.value }}
{% endfor %}Description:
 {{ description }}
 Built from {{ githash }} by {{ buildurl }}
{% endautoescape -%}
"#;

pub struct PolicyFileInput<'a> {
    pub key_filename: &'a str,
    pub key_id: &'a str,
    pub description: &'a str,
}

pub fn format_policy_file(input: &PolicyFileInput<'_>) -> Result<String> {
    let mut env = Environment::new();
    env.add_template("policy", POLICY_FILE_TEMPLATE)?;
    let tmpl = env.get_template("policy")?;
    tmpl.render(context! {
        key_filename => input.key_filename,
        key_id => input.key_id,
        description => input.description,
    })
    .context("rendering policy template")
}

#[derive(Debug, Clone)]
pub struct DebianControl {
    pub package_name: String,
    pub version: String,
    pub vendor: String,
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
    pub provides: Option<Vec<String>>,
    pub license: String,
    pub githash: String,
    pub buildurl: String,
}

#[derive(Serialize)]
struct Property<'a> {
    name: &'a str,
    value: String,
}

/// Appends an optional single-valued field, skipping it when blank.
///
/// `Vendor` and `Authors` have no meaningful empty form — emitting a bare
/// `Vendor:` would be a syntactically odd (and useless) control stanza — so a
/// blank value means "caller did not set this" and the field is omitted.
fn push_scalar(props: &mut Vec<Property<'static>>, name: &'static str, value: &str) {
    if !value.trim().is_empty() {
        props.push(Property {
            name,
            value: value.to_string(),
        });
    }
}

/// Appends a relationship field (`Depends`, `Conflicts`, …) as a
/// comma-separated list, skipping it when absent or empty.
///
/// Entries are trimmed and blanks dropped so that sloppy caller input (a
/// trailing comma, an empty element) cannot produce a malformed
/// `Depends: foo, ` line. The `", "` separator round-trips through
/// `content_verifier`, which splits on `,` and trims.
fn push_list(props: &mut Vec<Property<'static>>, name: &'static str, value: &Option<Vec<String>>) {
    let Some(items) = value else {
        return;
    };

    let joined = items
        .iter()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(", ");

    if !joined.is_empty() {
        props.push(Property {
            name,
            value: joined,
        });
    }
}

pub fn format_control_file(input: &DebianControl) -> Result<String> {
    let mut env = Environment::new();
    env.add_template("control", DEBIAN_CONTROL_FILE_TEMPLATE)?;
    let tmpl = env.get_template("control")?;

    // The twelve fields that were always emitted stay unconditional, blank or
    // not, so packages built before this change keep byte-identical stanzas
    // (`Installed-Size:` and `Source:` are routinely empty in practice).
    // Everything added here is emitted only when the caller supplied it.
    let mut properties: Vec<Property> = vec![
        Property {
            name: "Package",
            value: input.package_name.clone(),
        },
        Property {
            name: "Version",
            value: input.version.clone(),
        },
        Property {
            name: "Architecture",
            value: input.architecture.clone(),
        },
        Property {
            name: "Maintainer",
            value: input.package_maintainer.clone(),
        },
    ];

    push_scalar(&mut properties, "Vendor", &input.vendor);
    push_scalar(&mut properties, "Authors", &input.package_authors);

    properties.extend([
        Property {
            name: "Section",
            value: input.package_section.clone(),
        },
        Property {
            name: "Priority",
            value: input.package_priority.clone(),
        },
        Property {
            name: "Homepage",
            value: input.package_homepage.clone(),
        },
        Property {
            name: "Installed-Size",
            value: input.package_installed_size.clone(),
        },
    ]);

    // Relationship fields follow Installed-Size, per Debian convention.
    push_list(&mut properties, "Depends", &input.depends);
    push_list(&mut properties, "Pre-Depends", &input.pre_depends);
    push_list(&mut properties, "Recommends", &input.recommended_depends);
    push_list(&mut properties, "Suggests", &input.suggested_depends);
    push_list(&mut properties, "Conflicts", &input.conflicts);
    push_list(&mut properties, "Replaces", &input.replaces);
    push_list(&mut properties, "Provides", &input.provides);

    properties.extend([
        Property {
            name: "Source",
            value: input.package_source.clone(),
        },
        Property {
            name: "Suite",
            value: input.suite.clone(),
        },
        Property {
            name: "Codename",
            value: input.codename.clone(),
        },
        Property {
            name: "License",
            value: input.license.clone(),
        },
    ]);

    tmpl.render(context! {
        description => &input.package_description,
        githash => &input.githash,
        buildurl => &input.buildurl,
        properties => properties,
    })
    .context("rendering control template")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A control input with every optional field unset, so each test can set
    /// just the one it cares about.
    fn bare() -> DebianControl {
        DebianControl {
            package_name: "mina-daemon".into(),
            version: "1.2.3".into(),
            vendor: String::new(),
            package_authors: String::new(),
            package_maintainer: "O(1)Labs <build@o1labs.org>".into(),
            package_description: "Mina daemon".into(),
            package_section: "base".into(),
            package_priority: "optional".into(),
            package_homepage: "https://minaprotocol.com/".into(),
            package_installed_size: String::new(),
            package_source: String::new(),
            architecture: "amd64".into(),
            suite: "stable".into(),
            codename: "bookworm".into(),
            depends: None,
            suggested_depends: None,
            recommended_depends: None,
            pre_depends: None,
            conflicts: None,
            replaces: None,
            provides: None,
            license: "Apache-2.0".into(),
            githash: "abc1234".into(),
            buildurl: "https://ci.example/1".into(),
        }
    }

    fn field(out: &str, name: &str) -> Option<String> {
        out.lines()
            .find(|l| l.starts_with(&format!("{}: ", name)) || l.trim_end() == format!("{}:", name))
            .map(|l| l[name.len() + 1..].trim().to_string())
    }

    #[test]
    fn emits_depends_when_set() {
        let mut input = bare();
        input.depends = Some(vec!["libssl3".into(), "libgmp10".into()]);

        let out = format_control_file(&input).unwrap();

        assert_eq!(field(&out, "Depends").as_deref(), Some("libssl3, libgmp10"));
    }

    #[test]
    fn emits_every_relationship_field() {
        let mut input = bare();
        input.depends = Some(vec!["a".into()]);
        input.pre_depends = Some(vec!["b".into()]);
        input.recommended_depends = Some(vec!["c".into()]);
        input.suggested_depends = Some(vec!["d".into()]);
        input.conflicts = Some(vec!["e".into()]);
        input.replaces = Some(vec!["f".into()]);
        input.provides = Some(vec!["g".into()]);
        input.vendor = "O(1)Labs".into();
        input.package_authors = "Mina".into();

        let out = format_control_file(&input).unwrap();

        for (name, expected) in [
            ("Depends", "a"),
            ("Pre-Depends", "b"),
            ("Recommends", "c"),
            ("Suggests", "d"),
            ("Conflicts", "e"),
            ("Replaces", "f"),
            ("Provides", "g"),
            ("Vendor", "O(1)Labs"),
            ("Authors", "Mina"),
        ] {
            assert_eq!(
                field(&out, name).as_deref(),
                Some(expected),
                "{} wrong in:\n{}",
                name,
                out
            );
        }
    }

    #[test]
    fn omits_unset_optional_fields() {
        let out = format_control_file(&bare()).unwrap();

        for name in [
            "Depends",
            "Pre-Depends",
            "Recommends",
            "Suggests",
            "Conflicts",
            "Replaces",
            "Provides",
            "Vendor",
            "Authors",
        ] {
            assert!(
                !out.contains(&format!("{}:", name)),
                "{} should be absent from:\n{}",
                name,
                out
            );
        }
    }

    /// An empty list is "caller set it to nothing", which must not produce a
    /// bare `Depends:` — dpkg rejects a relationship field with no value.
    #[test]
    fn omits_empty_and_blank_only_lists() {
        let mut input = bare();
        input.depends = Some(vec![]);
        input.conflicts = Some(vec!["".into(), "   ".into()]);

        let out = format_control_file(&input).unwrap();

        assert!(!out.contains("Depends:"), "got:\n{}", out);
        assert!(!out.contains("Conflicts:"), "got:\n{}", out);
    }

    #[test]
    fn trims_entries_and_drops_blanks() {
        let mut input = bare();
        input.depends = Some(vec!["  libssl3 ".into(), "".into(), "libgmp10".into()]);

        let out = format_control_file(&input).unwrap();

        assert_eq!(field(&out, "Depends").as_deref(), Some("libssl3, libgmp10"));
    }

    /// The twelve originally-emitted fields must keep appearing even when
    /// blank, so existing packages keep byte-identical stanzas.
    #[test]
    fn always_emits_the_original_fields() {
        let out = format_control_file(&bare()).unwrap();

        for name in [
            "Package",
            "Version",
            "Architecture",
            "Maintainer",
            "Section",
            "Priority",
            "Homepage",
            "Installed-Size",
            "Source",
            "Suite",
            "Codename",
            "License",
        ] {
            assert!(
                out.contains(&format!("{}:", name)),
                "{} missing from:\n{}",
                name,
                out
            );
        }
    }

    /// The separator this writes must survive the reader in `content_verifier`,
    /// which splits on `,` and trims — otherwise `build` and `verify content`
    /// disagree about the same package.
    #[test]
    fn list_separator_round_trips_through_the_verifier_split() {
        let mut input = bare();
        input.depends = Some(vec!["libssl3".into(), "libgmp10".into(), "tzdata".into()]);

        let out = format_control_file(&input).unwrap();
        let rendered = field(&out, "Depends").unwrap();

        let parsed: Vec<String> = rendered
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();

        assert_eq!(parsed, vec!["libssl3", "libgmp10", "tzdata"]);
    }
}
