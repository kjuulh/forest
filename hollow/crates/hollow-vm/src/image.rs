//! Rootfs image integrity verification.
//!
//! Every `.ext4` rootfs ships with a sidecar `.ext4.sha256` file containing
//! the hex-encoded SHA-256 of the image, written by the build pipeline. Before
//! launching a VM we re-hash the image and compare; a mismatch means someone
//! either (a) shipped a corrupt image or (b) replaced the image on the host
//! between build and launch. Either way, refuse to boot it.
//!
//! Build-time pinning of the upstream artefacts that *produce* the image
//! (Alpine digest, tofu zip, provider zips) covers the supply chain up to
//! the build host. This module covers the gap between "image was built
//! correctly" and "image hasn't been swapped on the host since".

use std::path::{Path, PathBuf};

use anyhow::{Context, bail};
use sha2::{Digest, Sha256};

/// Path to the sidecar checksum file: `foo.ext4` → `foo.ext4.sha256`.
fn sidecar_path(image: &Path) -> PathBuf {
    let mut s = image.as_os_str().to_owned();
    s.push(".sha256");
    PathBuf::from(s)
}

/// Verify the image's SHA-256 matches the sidecar. The sidecar must contain
/// just the hex digest as its first whitespace-separated token (the
/// canonical `sha256sum` output format also works — we only read the first
/// token).
pub fn verify_digest(image: &Path) -> anyhow::Result<()> {
    let sidecar = sidecar_path(image);
    let raw = std::fs::read_to_string(&sidecar).with_context(|| {
        format!(
            "missing checksum sidecar {} — re-bootstrap the test host or rebuild the image",
            sidecar.display()
        )
    })?;
    let expected = raw
        .split_whitespace()
        .next()
        .unwrap_or("")
        .trim()
        .to_lowercase();
    if expected.len() != 64 || !expected.chars().all(|c| c.is_ascii_hexdigit()) {
        bail!(
            "{}: sidecar contents are not a sha256 hex digest (got {:?})",
            sidecar.display(),
            expected,
        );
    }

    let actual = sha256_file(image)
        .with_context(|| format!("hash {}", image.display()))?;
    if actual != expected {
        bail!(
            "rootfs digest mismatch for {}\n  expected (from {}): {expected}\n  actual:                {actual}",
            image.display(),
            sidecar.display(),
        );
    }
    tracing::debug!(
        path = %image.display(),
        sha256 = %actual,
        "rootfs digest verified"
    );
    Ok(())
}

fn sha256_file(path: &Path) -> std::io::Result<String> {
    let mut hasher = Sha256::new();
    let mut f = std::fs::File::open(path)?;
    std::io::copy(&mut f, &mut hasher)?;
    Ok(hex::encode(hasher.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verify_digest_accepts_matching_sidecar() {
        let dir = tempdir();
        let image = dir.join("test.ext4");
        let sidecar = dir.join("test.ext4.sha256");
        std::fs::write(&image, b"hello world").unwrap();
        // sha256("hello world") = b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9
        std::fs::write(
            &sidecar,
            "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9\n",
        )
        .unwrap();
        verify_digest(&image).unwrap();
    }

    #[test]
    fn verify_digest_accepts_sha256sum_format() {
        let dir = tempdir();
        let image = dir.join("test.ext4");
        let sidecar = dir.join("test.ext4.sha256");
        std::fs::write(&image, b"hello world").unwrap();
        // `sha256sum` writes "<hash>  <filename>"
        std::fs::write(
            &sidecar,
            "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9  test.ext4\n",
        )
        .unwrap();
        verify_digest(&image).unwrap();
    }

    #[test]
    fn verify_digest_rejects_mismatch() {
        let dir = tempdir();
        let image = dir.join("test.ext4");
        let sidecar = dir.join("test.ext4.sha256");
        std::fs::write(&image, b"hello world").unwrap();
        std::fs::write(&sidecar, "deadbeef".repeat(8) + "\n").unwrap();
        let err = verify_digest(&image).unwrap_err().to_string();
        assert!(err.contains("digest mismatch"), "got: {err}");
    }

    #[test]
    fn verify_digest_rejects_missing_sidecar() {
        let dir = tempdir();
        let image = dir.join("test.ext4");
        std::fs::write(&image, b"hello world").unwrap();
        let err = verify_digest(&image).unwrap_err().to_string();
        assert!(err.contains("missing checksum sidecar"), "got: {err}");
    }

    #[test]
    fn verify_digest_rejects_garbage_sidecar() {
        let dir = tempdir();
        let image = dir.join("test.ext4");
        let sidecar = dir.join("test.ext4.sha256");
        std::fs::write(&image, b"hello").unwrap();
        std::fs::write(&sidecar, "this is not a hash").unwrap();
        let err = verify_digest(&image).unwrap_err().to_string();
        assert!(err.contains("not a sha256"), "got: {err}");
    }

    /// Tiny tempdir helper — avoids pulling in the `tempfile` crate just for
    /// these unit tests. Cleans up via Drop on scope exit (best-effort).
    fn tempdir() -> Guard {
        let mut p = std::env::temp_dir();
        p.push(format!("hollow-vm-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&p).unwrap();
        Guard(p)
    }

    struct Guard(PathBuf);
    impl Guard {
        fn join(&self, name: &str) -> PathBuf {
            self.0.join(name)
        }
    }
    impl Drop for Guard {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}
