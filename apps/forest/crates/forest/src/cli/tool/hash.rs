//! `forest tool hash <url> [--archive zip] [--binary-in-archive <path>]`
//!
//! Downloads an upstream URL, computes the archive sha256 and (if extraction
//! is requested) the inner binary's sha256. No network shortcuts — uses the
//! same reqwest client the runtime will (https-only).

use clap::Args;
use sha2::{Digest, Sha256};

use crate::global::extract;
use crate::state::State;

#[derive(Args)]
pub struct ToolHashCommand {
    /// https:// URL to download.
    url: String,

    /// Archive format. `none` = the URL serves a bare executable.
    #[arg(long, default_value = "none")]
    archive: String,

    /// Path within the archive to the binary (required iff archive ≠ none).
    #[arg(long)]
    binary_in_archive: Option<String>,
}

impl ToolHashCommand {
    pub async fn execute(&self, _state: &State) -> anyhow::Result<()> {
        if !self.url.starts_with("https://") {
            anyhow::bail!("url must be https:// (got {})", self.url);
        }

        let body = reqwest::Client::builder()
            .use_rustls_tls()
            .redirect(reqwest::redirect::Policy::custom(|attempt| {
                if attempt.url().scheme() != "https" {
                    attempt.error("non-https redirect refused")
                } else if attempt.previous().len() >= 5 {
                    attempt.error("too many redirects")
                } else {
                    attempt.follow()
                }
            }))
            .build()?
            .get(&self.url)
            .send()
            .await?
            .error_for_status()?
            .bytes()
            .await?;

        let archive_sha = hex::encode(Sha256::digest(&body));
        println!("archive_sha256: {archive_sha}");

        if self.archive == "none" {
            // For `archive: "none"`, the manifest's `sha256` equals the
            // archive bytes (there's no extraction step).
            println!("binary_sha256:  {archive_sha}");
            return Ok(());
        }

        let target = self
            .binary_in_archive
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("--binary-in-archive required for archive != none"))?;
        let _canon = extract::canonicalise(target).map_err(|e| {
            anyhow::anyhow!("invalid --binary-in-archive: {e:?}")
        })?;

        let inner = extract_inner(&body, &self.archive, target)?;
        let binary_sha = hex::encode(Sha256::digest(&inner));
        println!("binary_sha256:  {binary_sha}");
        Ok(())
    }
}

fn extract_inner(body: &[u8], archive: &str, target: &str) -> anyhow::Result<Vec<u8>> {
    match archive {
        "tar.gz" => extract_tar_gz(body, target),
        "tar.xz" => anyhow::bail!("tar.xz not yet wired"),
        "tar.zst" => anyhow::bail!("tar.zst not yet wired"),
        "zip" => extract_zip(body, target),
        other => anyhow::bail!("unsupported archive: {other}"),
    }
}

fn extract_tar_gz(body: &[u8], target: &str) -> anyhow::Result<Vec<u8>> {
    use flate2::read::GzDecoder;
    use std::io::Read;
    use tar::Archive;

    let gz = GzDecoder::new(body);
    let mut archive = Archive::new(gz);
    let mut entries = Vec::new();
    for entry in archive.entries()? {
        let mut entry = entry?;
        let path = entry.path()?.to_string_lossy().into_owned();
        let mut buf = Vec::new();
        entry.read_to_end(&mut buf)?;
        entries.push((path, buf));
    }
    let names: Vec<String> = entries.iter().map(|(p, _)| p.clone()).collect();
    let idx = extract::select(&names, target)
        .map_err(|e| anyhow::anyhow!("could not select {target} from archive: {e:?}"))?;
    Ok(entries.swap_remove(idx).1)
}

fn extract_zip(body: &[u8], target: &str) -> anyhow::Result<Vec<u8>> {
    use std::io::{Cursor, Read};
    let mut archive = zip::ZipArchive::new(Cursor::new(body))?;
    let mut entries = Vec::new();
    for i in 0..archive.len() {
        let mut f = archive.by_index(i)?;
        let name = f.name().to_string();
        if f.is_dir() {
            continue;
        }
        let mut buf = Vec::new();
        f.read_to_end(&mut buf)?;
        entries.push((name, buf));
    }
    let names: Vec<String> = entries.iter().map(|(p, _)| p.clone()).collect();
    let idx = extract::select(&names, target)
        .map_err(|e| anyhow::anyhow!("could not select {target} from archive: {e:?}"))?;
    Ok(entries.swap_remove(idx).1)
}
