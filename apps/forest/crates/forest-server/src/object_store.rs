use anyhow::Context;
use s3::creds::Credentials;
use s3::{Bucket, Region};

/// S3-compatible object storage for binary data (component binaries, artifact tarballs).
#[derive(Clone)]
pub struct ObjectStore {
    bucket: Box<Bucket>,
}

impl ObjectStore {
    /// Create a new ObjectStore from environment variables.
    ///
    /// Required env vars: S3_ENDPOINT, S3_BUCKET, S3_ACCESS_KEY, S3_SECRET_KEY, S3_REGION
    pub fn from_env() -> anyhow::Result<Self> {
        let endpoint = std::env::var("S3_ENDPOINT").context("S3_ENDPOINT not set")?;
        let bucket_name = std::env::var("S3_BUCKET").context("S3_BUCKET not set")?;
        let access_key = std::env::var("S3_ACCESS_KEY").context("S3_ACCESS_KEY not set")?;
        let secret_key = std::env::var("S3_SECRET_KEY").context("S3_SECRET_KEY not set")?;
        let region_name = std::env::var("S3_REGION").unwrap_or_else(|_| "us-east-1".to_string());

        let region = Region::Custom {
            region: region_name,
            endpoint,
        };

        let credentials =
            Credentials::new(Some(&access_key), Some(&secret_key), None, None, None)
                .context("failed to create S3 credentials")?;

        let bucket = Bucket::new(&bucket_name, region, credentials)
            .context("failed to create S3 bucket client")?
            .with_path_style();

        Ok(Self { bucket })
    }

    /// Store an object.
    pub async fn put(&self, key: &str, data: &[u8]) -> anyhow::Result<()> {
        self.bucket
            .put_object(key, data)
            .await
            .with_context(|| format!("failed to put object: {key}"))?;
        Ok(())
    }

    /// Retrieve an object.
    pub async fn get(&self, key: &str) -> anyhow::Result<Vec<u8>> {
        let response = self
            .bucket
            .get_object(key)
            .await
            .with_context(|| format!("failed to get object: {key}"))?;
        Ok(response.to_vec())
    }

    /// Delete an object.
    pub async fn delete(&self, key: &str) -> anyhow::Result<()> {
        self.bucket
            .delete_object(key)
            .await
            .with_context(|| format!("failed to delete object: {key}"))?;
        Ok(())
    }

    /// Check if an object exists.
    pub async fn exists(&self, key: &str) -> anyhow::Result<bool> {
        match self.bucket.head_object(key).await {
            Ok(_) => Ok(true),
            Err(s3::error::S3Error::HttpFailWithBody(404, _)) => Ok(false),
            Err(e) => Err(anyhow::anyhow!("failed to check object existence: {e}")),
        }
    }
}

/// Key builders for consistent S3 path naming.
///
/// All inputs are sanitized to prevent path traversal attacks.
pub mod keys {
    /// Sanitize a path segment — remove `.`, `..`, `/`, `\`, and null bytes.
    fn sanitize(s: &str) -> String {
        s.replace(['/', '\\', '\0'], "")
            .replace("..", "")
            .trim_matches('.')
            .to_string()
    }

    /// Component binary key: `components/{org}/{name}/{version}/bin/{os}_{arch}`
    pub fn component_binary(org: &str, name: &str, version: &str, os: &str, arch: &str) -> String {
        format!(
            "components/{}/{}/{}/bin/{}_{}",
            sanitize(org), sanitize(name), sanitize(version), sanitize(os), sanitize(arch)
        )
    }

    /// Component file key: `components/{org}/{name}/{version}/files/{file_path}`
    pub fn component_file(org: &str, name: &str, version: &str, file_path: &str) -> String {
        // file_path may contain subdirectories — sanitize each segment
        let safe_path: String = file_path
            .split('/')
            .filter(|s| !s.is_empty() && *s != "." && *s != "..")
            .map(|s| s.replace(['\\', '\0'], ""))
            .collect::<Vec<_>>()
            .join("/");
        format!(
            "components/{}/{}/{}/files/{}",
            sanitize(org), sanitize(name), sanitize(version), safe_path
        )
    }

    /// Artifact tarball key: `artifacts/{artifact_id}.tar.gz`
    pub fn artifact_tarball(artifact_id: &str) -> String {
        format!("artifacts/{}.tar.gz", sanitize(artifact_id))
    }

    /// Extracted artifact file key: `artifacts/{artifact_id}/files/{env}/{destination}/{file_name}`
    /// Empty segments are filtered out to prevent double-slash paths.
    pub fn artifact_file(
        artifact_id: &str,
        env: &str,
        destination: &str,
        file_name: &str,
    ) -> String {
        let segments: Vec<String> = [
            sanitize(artifact_id),
            "files".to_string(),
            sanitize(env),
            sanitize(destination),
            sanitize(file_name),
        ]
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect();

        format!("artifacts/{}", segments.join("/"))
    }

    /// Component manifest key: `components/{org}/{name}/{version}/manifest.json`
    pub fn component_manifest(org: &str, name: &str, version: &str) -> String {
        format!(
            "components/{}/{}/{}/manifest.json",
            sanitize(org), sanitize(name), sanitize(version)
        )
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn test_sanitize_path_traversal() {
            assert_eq!(sanitize("../../../etc/passwd"), "etcpasswd");
            assert_eq!(sanitize("normal-name"), "normal-name");
            assert_eq!(sanitize("has/slash"), "hasslash");
            assert_eq!(sanitize("has\\backslash"), "hasbackslash");
            assert_eq!(sanitize(".."), "");
        }

        #[test]
        fn test_component_file_path_traversal() {
            let key = component_file("org", "name", "1.0.0", "../../etc/passwd");
            assert!(!key.contains(".."));
            assert!(key.starts_with("components/org/name/1.0.0/files/"));
        }
    }
}
