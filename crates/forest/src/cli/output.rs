use std::fmt::Write;

use serde::Serialize;
use tabled::{Table, Tabled, settings::Style};

#[derive(clap::ValueEnum, Clone, Copy, Default, Debug, PartialEq, Eq)]
pub enum OutputFormat {
    /// Human-readable table with rounded borders (default).
    #[default]
    Pretty,
    /// Tab-separated, no headers — for piping to other commands.
    Text,
    /// Just the first column of each row, one per line — perfect for `xargs`.
    /// Example: `forest global list --format name | xargs -L1 forest global which`
    Name,
    /// JSON array of typed rows.
    Json,
}

pub fn render<T: Tabled + Serialize>(format: &OutputFormat, rows: &[T]) -> String {
    match format {
        OutputFormat::Pretty if rows.len() == 1 => render_detail(&rows[0]),
        OutputFormat::Pretty => {
            let mut table = Table::new(rows);
            table.with(Style::rounded());
            format!("{table}\n")
        }
        OutputFormat::Text => {
            let mut out = String::new();
            for row in rows {
                let fields: Vec<String> = row
                    .fields()
                    .into_iter()
                    .map(|f| f.into_owned())
                    .collect();
                out.push_str(&fields.join("\t"));
                out.push('\n');
            }
            out
        }
        OutputFormat::Name => {
            let mut out = String::new();
            for row in rows {
                let mut fields = row.fields().into_iter();
                if let Some(first) = fields.next() {
                    out.push_str(&first);
                    out.push('\n');
                }
            }
            out
        }
        OutputFormat::Json => {
            format!("{}\n", serde_json::to_string_pretty(rows).unwrap_or_default())
        }
    }
}

fn render_detail<T: Tabled>(item: &T) -> String {
    let headers = T::headers();
    let fields = item.fields();

    let label_width = headers.iter().map(|h| h.len()).max().unwrap_or(0);

    let mut out = String::new();
    for (header, value) in headers.iter().zip(fields.iter()) {
        let _ = writeln!(out, "{:>width$}: {}", header, value, width = label_width);
    }
    out.push('\n');
    out
}
