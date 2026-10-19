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

pub const SHIM_MARKER: &str = "# forest shim — do not edit";

pub fn shim_script_for(qref: &QualifiedRef) -> String {
    format!(
        "#!/bin/sh\n{SHIM_MARKER}\nexec forest global run {}/{} --as \"${{0##*/}}\" -- \"$@\"\n",
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
exec forest global run cuteorg/ripgrep --as \"${0##*/}\" -- \"$@\"
";
        assert_eq!(script, expected);
    }

    #[test]
    fn renders_canonical_shim_for_hello() {
        let qref = QualifiedRef::new("cuteorg", "forest-hello");
        let script = shim_script_for(&qref);
        assert_eq!(
            script,
            "#!/bin/sh\n# forest shim — do not edit\nexec forest global run cuteorg/forest-hello --as \"${0##*/}\" -- \"$@\"\n"
        );
    }

    #[test]
    fn output_starts_with_posix_shebang() {
        let s = shim_script_for(&QualifiedRef::new("o", "n"));
        assert!(s.starts_with("#!/bin/sh\n"), "got: {s:?}");
    }

    #[test]
    fn second_line_is_the_marker() {
        let s = shim_script_for(&QualifiedRef::new("o", "n"));
        let mut lines = s.lines();
        let _shebang = lines.next();
        assert_eq!(lines.next(), Some(SHIM_MARKER));
    }

    #[test]
    fn body_uses_exec_with_double_dash_argv_separator() {
        let s = shim_script_for(&QualifiedRef::new("cuteorg", "ripgrep"));
        assert!(
            s.contains("exec forest global run cuteorg/ripgrep --as \"${0##*/}\" -- \"$@\""),
            "missing canonical exec line: {s:?}"
        );
    }

    #[test]
    fn output_is_deterministic() {
        let qref = QualifiedRef::new("cuteorg", "ripgrep");
        let a = shim_script_for(&qref);
        let b = shim_script_for(&qref);
        assert_eq!(a, b);
    }

    #[test]
    fn ends_with_newline() {
        let s = shim_script_for(&QualifiedRef::new("o", "n"));
        assert!(s.ends_with('\n'), "must end with newline: {s:?}");
    }

    #[test]
    fn body_forwards_the_name_the_shim_was_invoked_as() {
        let s = shim_script_for(&QualifiedRef::new("cuteorg", "ripgrep"));
        assert!(s.contains("--as \"${0##*/}\""), "got: {s:?}");
        assert!(
            !s.contains("basename"),
            "should not fork a subprocess: {s:?}"
        );
    }

    #[test]
    fn invoked_name_is_forwarded_before_the_argv_separator() {
        let s = shim_script_for(&QualifiedRef::new("o", "n"));
        let line = s.lines().nth(2).unwrap();
        let as_at = line.find("--as").unwrap();
        let sep_at = line.find(" -- ").unwrap();
        assert!(as_at < sep_at, "got: {line:?}");
    }

    #[test]
    fn body_quotes_argv_to_preserve_word_boundaries() {
        let s = shim_script_for(&QualifiedRef::new("o", "n"));
        assert!(s.contains("\"$@\""), "shim must quote $@: {s:?}");
    }
}
