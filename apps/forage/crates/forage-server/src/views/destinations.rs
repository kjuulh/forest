use std::collections::{BTreeMap, BTreeSet};

use forage_core::platform::{Destination, DestinationType, DestinationTypeInfo, Environment};
use forage_ui::chrome::Chrome;
use forage_ui::format::urlencode;

pub struct DestinationsPage {
    pub chrome: Chrome,
    pub org: String,
    pub csrf_token: String,
    pub is_admin: bool,
    pub environments: Vec<EnvironmentGroup>,
    pub orphans: Vec<DestinationRow>,
    pub types: Vec<DestinationTypeInfo>,
    pub script: String,
}

pub struct EnvironmentGroup {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub sort_order: i32,
    pub destinations: Vec<DestinationRow>,
}

pub struct DestinationRow {
    pub name: String,
    pub environment: String,
    pub dest_type: Option<DestinationType>,
    pub metadata_json: String,
    pub sensitive_keys: Vec<String>,
    pub sensitive_keys_json: String,
    pub key_count: usize,
}

impl DestinationRow {
    pub fn new(d: &Destination) -> Self {
        let metadata: BTreeMap<&String, &String> = d.metadata.iter().collect();
        Self {
            name: d.name.clone(),
            environment: d.environment.clone(),
            dest_type: d.dest_type.clone(),
            metadata_json: serde_json::to_string(&metadata).unwrap_or_else(|_| "{}".into()),
            sensitive_keys: d.sensitive_keys.clone(),
            sensitive_keys_json: serde_json::to_string(&d.sensitive_keys)
                .unwrap_or_else(|_| "[]".into()),
            key_count: d.metadata.len() + d.sensitive_keys.len(),
        }
    }

    pub fn type_org(&self) -> &str {
        self.dest_type
            .as_ref()
            .map(|t| t.organisation.as_str())
            .unwrap_or_default()
    }

    pub fn type_name(&self) -> &str {
        self.dest_type
            .as_ref()
            .map(|t| t.name.as_str())
            .unwrap_or_default()
    }

    pub fn type_version(&self) -> String {
        self.dest_type
            .as_ref()
            .map(|t| t.version.to_string())
            .unwrap_or_default()
    }

    pub fn shown_type(&self) -> Option<&DestinationType> {
        shown_type(&self.dest_type)
    }
}

pub fn shown_type(dest_type: &Option<DestinationType>) -> Option<&DestinationType> {
    dest_type.as_ref().filter(|t| !t.name.is_empty())
}

impl EnvironmentGroup {
    pub fn new(env: &Environment, destinations: &[Destination]) -> Self {
        Self {
            id: env.id.clone(),
            name: env.name.clone(),
            description: env.description.clone().filter(|d| !d.is_empty()),
            sort_order: env.sort_order,
            destinations: destinations
                .iter()
                .filter(|d| d.environment == env.name)
                .map(DestinationRow::new)
                .collect(),
        }
    }
}

pub fn orphans(environments: &[Environment], destinations: &[Destination]) -> Vec<DestinationRow> {
    let known: BTreeSet<&str> = environments.iter().map(|e| e.name.as_str()).collect();
    destinations
        .iter()
        .filter(|d| !known.contains(d.environment.as_str()))
        .map(DestinationRow::new)
        .collect()
}

pub struct DestinationDetailPage {
    pub chrome: Chrome,
    pub org: String,
    pub csrf_token: String,
    pub is_admin: bool,
    pub name: String,
    pub environment: String,
    pub dest_type: Option<DestinationType>,
    pub entries: Vec<MetadataEntry>,
    pub hidden_count: usize,
    pub declared_sensitive_json: String,
    pub script: String,
}

pub struct MetadataEntry {
    pub key: String,
    pub value: String,
    pub sensitive: bool,
    pub type_sensitive: bool,
}

impl DestinationDetailPage {
    pub fn entries(
        dest: &Destination,
        types: &[DestinationTypeInfo],
    ) -> (Vec<MetadataEntry>, String) {
        let type_sensitive: BTreeSet<&str> = dest
            .dest_type
            .as_ref()
            .and_then(|dt| {
                types
                    .iter()
                    .find(|t| t.organisation == dt.organisation && t.name == dt.name)
            })
            .map(|t| {
                t.fields
                    .iter()
                    .filter(|f| f.sensitive)
                    .map(|f| f.name.as_str())
                    .collect()
            })
            .unwrap_or_default();
        let declared: Vec<&String> = dest
            .sensitive_keys
            .iter()
            .filter(|k| !type_sensitive.contains(k.as_str()))
            .collect();
        let mut entries: Vec<MetadataEntry> = dest
            .metadata
            .iter()
            .map(|(k, v)| MetadataEntry {
                key: k.clone(),
                value: v.clone(),
                sensitive: false,
                type_sensitive: false,
            })
            .chain(dest.sensitive_keys.iter().map(|k| MetadataEntry {
                key: k.clone(),
                value: String::new(),
                sensitive: true,
                type_sensitive: type_sensitive.contains(k.as_str()),
            }))
            .collect();
        entries.sort_by(|a, b| a.key.cmp(&b.key));
        let declared_json = serde_json::to_string(&declared).unwrap_or_else(|_| "[]".into());
        (entries, declared_json)
    }
}

pub fn destinations_href(org: &str) -> String {
    format!("/orgs/{}/destinations", urlencode(org))
}

pub fn destination_href(org: &str, name: &str) -> String {
    format!("{}/detail?name={}", destinations_href(org), urlencode(name))
}

pub fn update_href(org: &str, name: &str) -> String {
    format!(
        "{}/detail/update?name={}",
        destinations_href(org),
        urlencode(name)
    )
}

pub fn reveal_href(org: &str, name: &str) -> String {
    format!(
        "{}/detail/reveal?name={}",
        destinations_href(org),
        urlencode(name)
    )
}

pub fn type_option(t: &DestinationTypeInfo) -> String {
    format!("{}/{}@{}", t.organisation, t.name, t.version)
}

pub fn type_option_label(t: &DestinationTypeInfo) -> String {
    let mut label = format!("{}/{} v{}", t.organisation, t.name, t.version);
    if !t.description.is_empty() {
        label.push_str(" — ");
        label.push_str(&t.description);
    }
    label
}

pub fn plural(n: usize, word: &str) -> String {
    if n == 1 {
        format!("{n} {word}")
    } else {
        format!("{n} {word}s")
    }
}
