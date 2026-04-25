//! `forest-contrib/render-template@v0.1` — directory-tree `{{var}}`
//! interpolation.
//!
//! Walks a source directory recursively. For each file, interpolates
//! `{{var}}` placeholders in:
//!   - file contents (UTF-8 only; binary files pass through verbatim)
//!   - path components (so `src/{{project}}/main.rs` lands at
//!     `dest/<actual project>/main.rs`)
//!
//! Unknown placeholders abort with an error so typos surface fast.
//! Executable bits are preserved on Unix.

#[allow(dead_code)]
mod forestgen;

use std::collections::HashMap;
use std::path::Path;

use forestgen::*;

struct Commands;

impl CommandHandler for Commands {
    async fn render_template(
        &self,
        _spec: &Spec,
        input: RenderTemplateInput,
    ) -> Result<RenderTemplateOutput, forest_sdk::Error> {
        let scalar_vars = scalarize(&input.vars).map_err(|e| {
            forest_sdk::Error::Handler(format!("vars contained non-scalar: {e}").into())
        })?;
        let count = render_dir(
            Path::new(&input.src),
            Path::new(&input.dest),
            &scalar_vars,
        )
        .map_err(|e| forest_sdk::Error::Handler(format!("render: {e:#}").into()))?;
        Ok(RenderTemplateOutput {
            files_rendered: count as i64,
            src: input.src,
            dest: input.dest,
        })
    }
}

/// Reduce arbitrary JSON values to strings. Strings pass through;
/// numbers/bools stringify; nulls become empty; objects/arrays error.
fn scalarize(
    vars: &HashMap<String, serde_json::Value>,
) -> anyhow::Result<HashMap<String, String>> {
    let mut out = HashMap::with_capacity(vars.len());
    for (k, v) in vars {
        let s = match v {
            serde_json::Value::String(s) => s.clone(),
            serde_json::Value::Number(n) => n.to_string(),
            serde_json::Value::Bool(b) => b.to_string(),
            serde_json::Value::Null => String::new(),
            other => anyhow::bail!("vars[{k}] must be scalar, got {other:?}"),
        };
        out.insert(k.clone(), s);
    }
    Ok(out)
}

fn render_dir(
    src: &Path,
    dest: &Path,
    vars: &HashMap<String, String>,
) -> anyhow::Result<usize> {
    if !src.is_dir() {
        anyhow::bail!("source is not a directory: {}", src.display());
    }
    std::fs::create_dir_all(dest)
        .map_err(|e| anyhow::anyhow!("create_dir_all {}: {e}", dest.display()))?;

    let mut count = 0usize;
    walk(src, dest, vars, &mut count)?;
    Ok(count)
}

fn walk(
    src: &Path,
    dest_root: &Path,
    vars: &HashMap<String, String>,
    count: &mut usize,
) -> anyhow::Result<()> {
    for entry in std::fs::read_dir(src)
        .map_err(|e| anyhow::anyhow!("read_dir {}: {e}", src.display()))?
    {
        let entry = entry?;
        let entry_path = entry.path();
        let file_type = entry.file_type()?;

        let raw_name = entry
            .file_name()
            .into_string()
            .map_err(|n| anyhow::anyhow!("non-UTF-8 filename: {n:?}"))?;
        let rendered_name = render_str(&raw_name, vars)?;
        let target = dest_root.join(&rendered_name);

        if file_type.is_dir() {
            std::fs::create_dir_all(&target)
                .map_err(|e| anyhow::anyhow!("create {}: {e}", target.display()))?;
            walk(&entry_path, &target, vars, count)?;
        } else if file_type.is_file() {
            let bytes = std::fs::read(&entry_path)
                .map_err(|e| anyhow::anyhow!("read {}: {e}", entry_path.display()))?;
            let rendered_bytes = match std::str::from_utf8(&bytes) {
                Ok(s) => render_str(s, vars)?.into_bytes(),
                Err(_) => bytes,
            };
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| anyhow::anyhow!("create {}: {e}", parent.display()))?;
            }
            std::fs::write(&target, rendered_bytes)
                .map_err(|e| anyhow::anyhow!("write {}: {e}", target.display()))?;

            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let src_mode = std::fs::metadata(&entry_path)?.permissions().mode();
                let mut perm = std::fs::metadata(&target)?.permissions();
                perm.set_mode(src_mode);
                std::fs::set_permissions(&target, perm)?;
            }

            *count += 1;
        }
        // Symlinks and other special files are skipped silently.
    }
    Ok(())
}

/// Render a single string, replacing every `{{var}}` (with optional
/// surrounding whitespace) with the matching scalar from `vars`.
/// Operates on `&str` byte indices; `{{` and `}}` are pure ASCII so
/// slicing on those boundaries is always UTF-8 safe.
fn render_str(input: &str, vars: &HashMap<String, String>) -> anyhow::Result<String> {
    let mut out = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(open) = rest.find("{{") {
        out.push_str(&rest[..open]);
        let after_open = &rest[open + 2..];
        let Some(close_rel) = after_open.find("}}") else {
            out.push_str(&rest[open..]);
            return Ok(out);
        };
        let key = after_open[..close_rel].trim();
        let value = vars
            .get(key)
            .ok_or_else(|| anyhow::anyhow!("unknown placeholder: {{{{ {key} }}}}"))?;
        out.push_str(value);
        rest = &after_open[close_rel + 2..];
    }
    out.push_str(rest);
    Ok(out)
}

fn main() {
    let router = ComponentRouter::new(Commands);
    forest_sdk::run_once(&router);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_scalar_placeholders() {
        let vars = HashMap::from([
            ("name".to_string(), "forest".to_string()),
            ("year".to_string(), "2026".to_string()),
        ]);
        let out = render_str("Hello {{name}} ({{ year }})", &vars).unwrap();
        assert_eq!(out, "Hello forest (2026)");
    }

    #[test]
    fn unknown_placeholder_errors() {
        let vars = HashMap::new();
        let err = render_str("hi {{missing}}", &vars).unwrap_err();
        assert!(err.to_string().contains("missing"));
    }

    #[test]
    fn unmatched_open_brace_passes_through() {
        let vars = HashMap::new();
        let out = render_str("hi {{ no close", &vars).unwrap();
        assert_eq!(out, "hi {{ no close");
    }
}
