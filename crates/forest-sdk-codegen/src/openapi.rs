use crate::errors::CodegenResult;

pub mod models {
    use std::collections::BTreeMap;

    use serde::{Deserialize, Serialize};

    /// Root OpenAPI 3.x document.
    #[derive(Debug, Clone, Deserialize, Serialize)]
    pub struct Document {
        pub openapi: String,
        pub info: Info,
        #[serde(default)]
        pub paths: BTreeMap<String, PathItem>,
        #[serde(default)]
        pub components: Option<Components>,
        #[serde(default)]
        pub servers: Option<Vec<Server>>,
        #[serde(default)]
        pub tags: Option<Vec<Tag>>,
    }

    #[derive(Debug, Clone, Deserialize, Serialize)]
    pub struct Info {
        pub title: String,
        pub version: String,
        #[serde(default)]
        pub description: Option<String>,
        #[serde(rename = "termsOfService", default)]
        pub terms_of_service: Option<String>,
        #[serde(default)]
        pub contact: Option<Contact>,
        #[serde(default)]
        pub license: Option<License>,
    }

    #[derive(Debug, Clone, Deserialize, Serialize)]
    pub struct Contact {
        #[serde(default)]
        pub name: Option<String>,
        #[serde(default)]
        pub url: Option<String>,
        #[serde(default)]
        pub email: Option<String>,
    }

    #[derive(Debug, Clone, Deserialize, Serialize)]
    pub struct License {
        pub name: String,
        #[serde(default)]
        pub url: Option<String>,
    }

    #[derive(Debug, Clone, Deserialize, Serialize)]
    pub struct Server {
        pub url: String,
        #[serde(default)]
        pub description: Option<String>,
    }

    #[derive(Debug, Clone, Deserialize, Serialize)]
    pub struct Tag {
        pub name: String,
        #[serde(default)]
        pub description: Option<String>,
    }

    #[derive(Debug, Clone, Deserialize, Serialize)]
    pub struct PathItem {
        #[serde(default)]
        pub summary: Option<String>,
        #[serde(default)]
        pub description: Option<String>,
        #[serde(default)]
        pub get: Option<Operation>,
        #[serde(default)]
        pub put: Option<Operation>,
        #[serde(default)]
        pub post: Option<Operation>,
        #[serde(default)]
        pub delete: Option<Operation>,
        #[serde(default)]
        pub options: Option<Operation>,
        #[serde(default)]
        pub head: Option<Operation>,
        #[serde(default)]
        pub patch: Option<Operation>,
        #[serde(default)]
        pub trace: Option<Operation>,
        #[serde(default)]
        pub parameters: Option<Vec<ParameterOrRef>>,
    }

    #[derive(Debug, Clone, Deserialize, Serialize)]
    pub struct Operation {
        #[serde(default)]
        pub summary: Option<String>,
        #[serde(default)]
        pub description: Option<String>,
        #[serde(rename = "operationId", default)]
        pub operation_id: Option<String>,
        #[serde(default)]
        pub tags: Option<Vec<String>>,
        #[serde(default)]
        pub parameters: Option<Vec<ParameterOrRef>>,
        #[serde(rename = "requestBody", default)]
        pub request_body: Option<RequestBodyOrRef>,
        #[serde(default)]
        pub responses: BTreeMap<String, ResponseOrRef>,
        #[serde(default)]
        pub deprecated: Option<bool>,
    }

    #[derive(Debug, Clone, Deserialize, Serialize)]
    pub struct Parameter {
        pub name: String,
        #[serde(rename = "in")]
        pub location: String,
        #[serde(default)]
        pub description: Option<String>,
        #[serde(default)]
        pub required: Option<bool>,
        #[serde(default)]
        pub deprecated: Option<bool>,
        #[serde(default)]
        pub schema: Option<SchemaOrRef>,
    }

    #[derive(Debug, Clone, Deserialize, Serialize)]
    #[serde(untagged)]
    pub enum ParameterOrRef {
        Ref(Reference),
        Parameter(Box<Parameter>),
    }

    #[derive(Debug, Clone, Deserialize, Serialize)]
    pub struct RequestBody {
        #[serde(default)]
        pub description: Option<String>,
        #[serde(default)]
        pub required: Option<bool>,
        #[serde(default)]
        pub content: BTreeMap<String, MediaType>,
    }

    #[derive(Debug, Clone, Deserialize, Serialize)]
    #[serde(untagged)]
    pub enum RequestBodyOrRef {
        Ref(Reference),
        RequestBody(RequestBody),
    }

    #[derive(Debug, Clone, Deserialize, Serialize)]
    pub struct Response {
        #[serde(default)]
        pub description: Option<String>,
        #[serde(default)]
        pub content: Option<BTreeMap<String, MediaType>>,
        #[serde(default)]
        pub headers: Option<BTreeMap<String, HeaderOrRef>>,
    }

    #[derive(Debug, Clone, Deserialize, Serialize)]
    #[serde(untagged)]
    pub enum ResponseOrRef {
        Ref(Reference),
        Response(Response),
    }

    #[derive(Debug, Clone, Deserialize, Serialize)]
    pub struct Header {
        #[serde(default)]
        pub description: Option<String>,
        #[serde(default)]
        pub required: Option<bool>,
        #[serde(default)]
        pub schema: Option<SchemaOrRef>,
    }

    #[derive(Debug, Clone, Deserialize, Serialize)]
    #[serde(untagged)]
    pub enum HeaderOrRef {
        Ref(Reference),
        Header(Box<Header>),
    }

    #[derive(Debug, Clone, Deserialize, Serialize)]
    pub struct MediaType {
        #[serde(default)]
        pub schema: Option<SchemaOrRef>,
    }

    #[derive(Debug, Clone, Deserialize, Serialize)]
    pub struct Components {
        #[serde(default)]
        pub schemas: BTreeMap<String, SchemaOrRef>,
        #[serde(default)]
        pub responses: Option<BTreeMap<String, ResponseOrRef>>,
        #[serde(default)]
        pub parameters: Option<BTreeMap<String, ParameterOrRef>>,
        #[serde(rename = "requestBodies", default)]
        pub request_bodies: Option<BTreeMap<String, RequestBodyOrRef>>,
        #[serde(default)]
        pub headers: Option<BTreeMap<String, HeaderOrRef>>,
    }

    /// Either an inline schema or a `$ref` reference.
    /// `Ref` is tried first so that `{"$ref": "..."}` objects are correctly
    /// distinguished from plain schemas (where all fields are optional).
    #[derive(Debug, Clone, Deserialize, Serialize)]
    #[serde(untagged)]
    pub enum SchemaOrRef {
        Ref(Reference),
        Schema(Box<Schema>),
    }

    #[derive(Debug, Clone, Deserialize, Serialize)]
    pub struct Reference {
        #[serde(rename = "$ref")]
        pub ref_path: String,
    }

    #[derive(Debug, Clone, Deserialize, Serialize)]
    pub struct Schema {
        #[serde(rename = "type", default)]
        pub schema_type: Option<String>,
        #[serde(default)]
        pub description: Option<String>,
        #[serde(default)]
        pub properties: Option<BTreeMap<String, SchemaOrRef>>,
        #[serde(default)]
        pub required: Option<Vec<String>>,
        #[serde(rename = "enum", default)]
        pub enum_values: Option<Vec<serde_json::Value>>,
        #[serde(rename = "additionalProperties", default)]
        pub additional_properties: Option<Box<SchemaOrRef>>,
        #[serde(rename = "allOf", default)]
        pub all_of: Option<Vec<SchemaOrRef>>,
        #[serde(rename = "oneOf", default)]
        pub one_of: Option<Vec<SchemaOrRef>>,
        #[serde(rename = "anyOf", default)]
        pub any_of: Option<Vec<SchemaOrRef>>,
        #[serde(default)]
        pub not: Option<Box<SchemaOrRef>>,
        #[serde(default)]
        pub items: Option<Box<SchemaOrRef>>,
        #[serde(default)]
        pub minimum: Option<serde_json::Number>,
        #[serde(default)]
        pub maximum: Option<serde_json::Number>,
        #[serde(rename = "exclusiveMinimum", default)]
        pub exclusive_minimum: Option<serde_json::Value>,
        #[serde(rename = "exclusiveMaximum", default)]
        pub exclusive_maximum: Option<serde_json::Value>,
        #[serde(default)]
        pub default: Option<serde_json::Value>,
        #[serde(default)]
        pub pattern: Option<String>,
        #[serde(default)]
        pub format: Option<String>,
        #[serde(default)]
        pub title: Option<String>,
        #[serde(rename = "readOnly", default)]
        pub read_only: Option<bool>,
        #[serde(rename = "writeOnly", default)]
        pub write_only: Option<bool>,
        #[serde(default)]
        pub nullable: Option<bool>,
        #[serde(default)]
        pub deprecated: Option<bool>,
        #[serde(rename = "minLength", default)]
        pub min_length: Option<u64>,
        #[serde(rename = "maxLength", default)]
        pub max_length: Option<u64>,
        #[serde(rename = "minItems", default)]
        pub min_items: Option<u64>,
        #[serde(rename = "maxItems", default)]
        pub max_items: Option<u64>,
        #[serde(rename = "uniqueItems", default)]
        pub unique_items: Option<bool>,
    }
}

pub fn parse(input: &str) -> CodegenResult<models::Document> {
    let doc: models::Document = serde_json::from_str(input)?;
    Ok(doc)
}
