use std::pin::Pin;

use anyhow::Context;
use bytes::Bytes;
use futures::Stream;
use s3::creds::Credentials;
use s3::{Bucket, Region};

/// A byte stream from object storage, with the `s3` crate's error type mapped
/// away so callers do not have to depend on it.
pub type ByteStream = Pin<Box<dyn Stream<Item = anyhow::Result<Bytes>> + Send>>;

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

        let credentials = Credentials::new(Some(&access_key), Some(&secret_key), None, None, None)
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

    /// Retrieve an object as a stream, without buffering it (DATA-505).
    ///
    /// [`Self::get`] pulls the whole object into a `Vec<u8>` before returning,
    /// which for a large component binary means the caller cannot send a single
    /// byte until the entire S3 GET has finished, and holds the whole artifact
    /// in memory while it does. This yields chunks as they arrive so a download
    /// handler can forward them straight to the client.
    pub async fn get_stream(&self, key: &str) -> anyhow::Result<ByteStream> {
        let response = self
            .bucket
            .get_object_stream(key)
            .await
            .with_context(|| format!("failed to stream object: {key}"))?;

        // The `fail-on-err` feature makes a non-2xx status an error before we
        // get here; this is belt and braces so a surprising status can never be
        // mistaken for object content.
        if !(200..300).contains(&response.status_code) {
            anyhow::bail!(
                "failed to stream object {key}: unexpected status {}",
                response.status_code
            );
        }

        let key = key.to_string();
        Ok(Box::pin(futures::StreamExt::map(
            response.bytes,
            move |chunk| chunk.with_context(|| format!("reading object stream: {key}")),
        )))
    }

    /// Stream `length` bytes of an object starting at `offset`, as one ranged
    /// GET, without buffering it.
    ///
    /// The stream yields exactly `length` bytes or ends in an error: a store
    /// that ignores the `Range` header (answering 200 with the whole object) or
    /// returns a short or long body never passes for the requested range.
    pub async fn get_range_stream(
        &self,
        key: &str,
        offset: u64,
        length: u64,
    ) -> anyhow::Result<ByteStream> {
        use s3::request::Request as _;

        anyhow::ensure!(length > 0, "empty range requested for {key}");
        let last = offset
            .checked_add(length - 1)
            .with_context(|| format!("range overflows for {key}"))?;
        let request = s3::request::tokio_backend::HyperRequest::new(
            &self.bucket,
            key,
            s3::command::Command::GetObjectRange {
                start: offset,
                end: Some(last),
            },
        )
        .await
        .with_context(|| format!("failed to build ranged request: {key}"))?;
        let response = request
            .response_data_to_stream()
            .await
            .with_context(|| format!("failed to stream object range: {key}"))?;

        if response.status_code != 206 {
            anyhow::bail!(
                "failed to stream object range {key} ({offset}+{length}): unexpected status {}",
                response.status_code
            );
        }

        Ok(Box::pin(exact_length(
            response.bytes,
            length,
            format!("{key} ({offset}+{length})"),
        )))
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

fn exact_length<S, E>(
    inner: S,
    length: u64,
    what: String,
) -> impl futures::Stream<Item = anyhow::Result<Bytes>> + Send
where
    S: futures::Stream<Item = Result<Bytes, E>> + Send + Unpin,
    E: std::error::Error + Send + Sync + 'static,
{
    use futures::StreamExt as _;

    futures::stream::unfold((inner, 0u64, false), move |(mut inner, seen, done)| {
        let what = what.clone();
        async move {
            if done {
                return None;
            }
            match inner.next().await {
                Some(Ok(part)) => {
                    let seen = seen + part.len() as u64;
                    if seen > length {
                        let e = anyhow::anyhow!(
                            "object range {what}: store returned more than {length} bytes"
                        );
                        return Some((Err(e), (inner, seen, true)));
                    }
                    Some((Ok(part), (inner, seen, false)))
                }
                Some(Err(e)) => Some((
                    Err(anyhow::Error::new(e).context(format!("reading object range {what}"))),
                    (inner, seen, true),
                )),
                None if seen == length => None,
                None => {
                    let e = anyhow::anyhow!(
                        "object range {what}: store ended after {seen} of {length} bytes"
                    );
                    Some((Err(e), (inner, seen, true)))
                }
            }
        }
    })
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
            sanitize(org),
            sanitize(name),
            sanitize(version),
            sanitize(os),
            sanitize(arch)
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
            sanitize(org),
            sanitize(name),
            sanitize(version),
            safe_path
        )
    }

    /// Artifact tarball key: `artifacts/{artifact_id}.tar.gz`
    pub fn artifact_tarball(artifact_id: &str) -> String {
        format!("artifacts/{}.tar.gz", sanitize(artifact_id))
    }

    /// Extracted artifact file key: `artifacts/{artifact_id}/files/{env}/{destination}/{file_name}`
    /// Empty segments are filtered out to prevent double-slash paths.
    /// Key for an artifact's file, derived from the artifact and the file's own
    /// path — and deliberately **not** from its env or destination.
    ///
    /// The old key mixed both in, which quietly made them immutable: correcting
    /// a mis-recorded destination moved the object's key, so the read went
    /// looking in the wrong place. That is what kept the truncated selector in
    /// `artifact_files.destination` un-fixable from the server, and pushed the
    /// decision back onto every client.
    ///
    /// `file_name` is already the file's full path within the artifact, so it is
    /// unique on its own. Segments are sanitized individually and rejoined,
    /// rather than flattened into one segment: flattening let `a/b` and `ab`
    /// collide, and with the old key so did two selectors differing only by a
    /// slash.
    pub fn artifact_file_by_name(artifact_id: &str, file_name: &str) -> String {
        let path: Vec<String> = file_name
            .split('/')
            .filter(|s| !s.is_empty() && *s != "." && *s != "..")
            .map(|s| s.replace(['\\', '\0'], ""))
            .filter(|s| !s.is_empty())
            .collect();

        format!(
            "artifacts/{}/files/{}",
            sanitize(artifact_id),
            path.join("/")
        )
    }

    /// The pre-`artifact_file_by_name` key. Still read, never written: artifacts
    /// uploaded before the change have their objects here.
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
            sanitize(org),
            sanitize(name),
            sanitize(version)
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

#[cfg(test)]
mod exact_length_tests {
    use futures::StreamExt as _;

    use super::*;

    async fn through(parts: Vec<&'static [u8]>, length: u64) -> anyhow::Result<Vec<u8>> {
        let inner = futures::stream::iter(
            parts
                .into_iter()
                .map(|p| Ok::<_, std::io::Error>(Bytes::from_static(p))),
        );
        let mut out = Vec::new();
        let mut stream = Box::pin(exact_length(inner, length, "k (0+n)".into()));
        while let Some(part) = stream.next().await {
            out.extend_from_slice(&part?);
        }
        Ok(out)
    }

    #[tokio::test]
    async fn passes_a_body_of_exactly_the_requested_length() {
        assert_eq!(through(vec![b"abc", b"de"], 5).await.unwrap(), b"abcde");
    }

    #[tokio::test]
    async fn a_short_body_is_an_error_not_a_clean_end() {
        let err = through(vec![b"abc"], 5).await.unwrap_err();
        assert!(err.to_string().contains("ended after 3 of 5"), "{err}");
    }

    #[tokio::test]
    async fn a_long_body_is_an_error() {
        let err = through(vec![b"abc", b"def"], 5).await.unwrap_err();
        assert!(err.to_string().contains("more than 5"), "{err}");
    }
}
