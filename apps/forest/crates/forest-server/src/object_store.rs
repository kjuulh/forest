use std::pin::Pin;

use anyhow::Context;
use bytes::Bytes;
use futures::Stream;
use s3::creds::Credentials;
use s3::{Bucket, Region};

pub type ByteStream = Pin<Box<dyn Stream<Item = anyhow::Result<Bytes>> + Send>>;

#[derive(Clone)]
pub struct ObjectStore {
    bucket: Box<Bucket>,
}

impl ObjectStore {
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

    pub async fn put(&self, key: &str, data: &[u8]) -> anyhow::Result<()> {
        self.bucket
            .put_object(key, data)
            .await
            .with_context(|| format!("failed to put object: {key}"))?;
        Ok(())
    }

    pub async fn get(&self, key: &str) -> anyhow::Result<Vec<u8>> {
        let response = self
            .bucket
            .get_object(key)
            .await
            .with_context(|| format!("failed to get object: {key}"))?;
        Ok(response.to_vec())
    }

    pub async fn get_stream(&self, key: &str) -> anyhow::Result<ByteStream> {
        let response = self
            .bucket
            .get_object_stream(key)
            .await
            .with_context(|| format!("failed to stream object: {key}"))?;

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

    pub async fn delete(&self, key: &str) -> anyhow::Result<()> {
        self.bucket
            .delete_object(key)
            .await
            .with_context(|| format!("failed to delete object: {key}"))?;
        Ok(())
    }

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

pub mod keys {
    fn sanitize(s: &str) -> String {
        s.replace(['/', '\\', '\0'], "")
            .replace("..", "")
            .trim_matches('.')
            .to_string()
    }

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

    pub fn component_file(org: &str, name: &str, version: &str, file_path: &str) -> String {
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

    pub fn artifact_tarball(artifact_id: &str) -> String {
        format!("artifacts/{}.tar.gz", sanitize(artifact_id))
    }

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
