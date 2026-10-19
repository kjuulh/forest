use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;

#[async_trait]
pub trait DnsResolver: Send + Sync {
    async fn lookup_txt(&self, name: &str) -> anyhow::Result<Vec<String>>;
}

pub struct HickoryResolver {
    inner: hickory_resolver::TokioAsyncResolver,
}

impl HickoryResolver {
    pub fn from_system() -> anyhow::Result<Self> {
        let resolver = hickory_resolver::TokioAsyncResolver::tokio_from_system_conf()?;
        Ok(Self { inner: resolver })
    }
}

#[async_trait]
impl DnsResolver for HickoryResolver {
    async fn lookup_txt(&self, name: &str) -> anyhow::Result<Vec<String>> {
        use hickory_resolver::error::ResolveErrorKind;
        match self.inner.txt_lookup(name).await {
            Ok(records) => Ok(records
                .iter()
                .flat_map(|r| {
                    r.iter()
                        .map(|chunk| String::from_utf8_lossy(chunk).into_owned())
                })
                .collect()),
            Err(e) => match e.kind() {
                ResolveErrorKind::NoRecordsFound { .. } => Ok(vec![]),
                _ => Err(anyhow::Error::from(e)),
            },
        }
    }
}

#[derive(Clone, Default)]
pub struct MockDnsResolver {
    records: Arc<Mutex<HashMap<String, Vec<String>>>>,
}

impl MockDnsResolver {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_txt(&self, name: &str, value: &str) {
        let mut m = self.records.lock().unwrap();
        m.entry(name.to_ascii_lowercase())
            .or_default()
            .push(value.to_string());
    }
}

#[async_trait]
impl DnsResolver for MockDnsResolver {
    async fn lookup_txt(&self, name: &str) -> anyhow::Result<Vec<String>> {
        let m = self.records.lock().unwrap();
        Ok(m.get(&name.to_ascii_lowercase())
            .cloned()
            .unwrap_or_default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn mock_returns_preloaded_records() {
        let r = MockDnsResolver::new();
        r.set_txt("_forest-verify.example.com", "abc123");
        let got = r.lookup_txt("_forest-verify.example.com").await.unwrap();
        assert_eq!(got, vec!["abc123"]);
    }

    #[tokio::test]
    async fn mock_unknown_name_is_empty() {
        let r = MockDnsResolver::new();
        let got = r.lookup_txt("unknown.example").await.unwrap();
        assert!(got.is_empty());
    }

    #[tokio::test]
    async fn mock_is_case_insensitive() {
        let r = MockDnsResolver::new();
        r.set_txt("_forest-verify.EXAMPLE.com", "abc");
        let got = r.lookup_txt("_forest-verify.example.COM").await.unwrap();
        assert_eq!(got, vec!["abc"]);
    }
}
