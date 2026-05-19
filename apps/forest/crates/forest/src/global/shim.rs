//! Shim script generator.
//!
//! Pure module — no I/O. Generates the POSIX shell script content for a
//! shim file as specified in TASKS/018-global-tools.md §1a.6.
//!
//! The shim filename is `shim_name` (file the user types on PATH); the body
//! always embeds the **qualified** `<org>/<name>` so resolution goes
//! through the upstream identity regardless of aliasing.

/// A qualified reference to a tool: `<organisation>/<name>`.
///
/// `version` is intentionally NOT part of the shim body. Version pinning
/// lives in `forest.cue` (`config.dependencies` or `config.org_catalog.X.pins`);
/// the shim defers to `forest global run` which reads that pin at invocation
/// time. This makes `forest global update` cheap (no shim rewrites for
/// version bumps).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QualifiedRef {
    pub organisation: String,
    pub name: String,
}

impl QualifiedRef {
    pub fn new(organisation: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            organisation: organisation.into(),
            name: name.into(),
        }
    }
}

/// The literal first line of every shim — used as the magic marker
/// `forest global sync` checks before deleting orphan shims (§1a.6).
pub const SHIM_MARKER: &str = "# forest shim — do not edit";

/// Render the POSIX shell script for a shim that, when executed, exec's
/// `forest global run <org>/<name>` with the caller's argv passed through.
///
/// Output is deterministic — same input always yields byte-identical output.
pub fn shim_script_for(qref: &QualifiedRef) -> String {
    format!(
        "#!/bin/sh\n{SHIM_MARKER}\nexec forest global run {}/{} -- \"$@\"\n",
        qref.organisation, qref.name,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_canonical_shim_for_ripgrep() {
        let qref = QualifiedRef::new("cuteorg", "ripgrep");
        let script = shim_script_for(&qref);
        let expected = "\
#!/bin/sh
# forest shim — do not edit
exec forest global run cuteorg/ripgrep -- \"$@\"
";
        assert_eq!(script, expected);
    }

    #[test]
    fn renders_canonical_shim_for_hello() {
        // The forest-hello example component.
        let qref = QualifiedRef::new("cuteorg", "forest-hello");
        let script = shim_script_for(&qref);
        assert_eq!(
            script,
            "#!/bin/sh\n# forest shim — do not edit\nexec forest global run cuteorg/forest-hello -- \"$@\"\n"
        );
    }

    #[test]
    fn output_starts_with_posix_shebang() {
        let s = shim_script_for(&QualifiedRef::new("o", "n"));
        assert!(s.starts_with("#!/bin/sh\n"), "got: {s:?}");
    }

    #[test]
    fn second_line_is_the_marker() {
        // §1a.6: `forest global sync` deletes a shim only if its first 2
        // lines start with the marker. Make sure the marker is on line 2.
        let s = shim_script_for(&QualifiedRef::new("o", "n"));
        let mut lines = s.lines();
        let _shebang = lines.next();
        assert_eq!(lines.next(), Some(SHIM_MARKER));
    }

    #[test]
    fn body_uses_exec_with_double_dash_argv_separator() {
        // §1a.6: shims must use the `--` separator so the underlying binary
        // can receive flags without forest's clap eating them.
        let s = shim_script_for(&QualifiedRef::new("cuteorg", "ripgrep"));
        assert!(
            s.contains("exec forest global run cuteorg/ripgrep -- \"$@\""),
            "missing canonical exec line: {s:?}"
        );
    }

    #[test]
    fn output_is_deterministic() {
        // P5 (downgraded to property test): same input -> byte-identical output.
        let qref = QualifiedRef::new("cuteorg", "ripgrep");
        let a = shim_script_for(&qref);
        let b = shim_script_for(&qref);
        assert_eq!(a, b);
    }

    #[test]
    fn ends_with_newline() {
        // POSIX scripts SHOULD end with a newline; some shells warn otherwise.
        let s = shim_script_for(&QualifiedRef::new("o", "n"));
        assert!(s.ends_with('\n'), "must end with newline: {s:?}");
    }

    #[test]
    fn body_quotes_argv_to_preserve_word_boundaries() {
        // `"$@"` (not `$@`) is required so spaces inside individual args
        // don't get re-split by the shell.
        let s = shim_script_for(&QualifiedRef::new("o", "n"));
        assert!(s.contains("\"$@\""), "shim must quote $@: {s:?}");
    }
}
