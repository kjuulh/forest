/// Language-agnostic intermediate representation for a Forest component.
///
/// One [`Module`] is produced per component and captures all the
/// Forest-specific semantics (component identity, spec, commands, hooks,
/// user-defined types) without any trace of OpenAPI syntax.
///
/// Root of the IR — one module per component.
#[derive(Debug, Clone)]
pub struct Module {
    pub spec: Spec,
    pub commands: Vec<Command>,
    pub hook_groups: Vec<HookGroup>,
    /// Named user-defined types (Port, HealthCheck, CPU, Memory, etc.)
    pub type_defs: Vec<TypeDef>,
}

/// Component identity metadata.
#[derive(Debug, Clone)]
pub struct Component {
    pub name: String,
    pub org: String,
    pub version: String,
}

/// The input specification.
#[derive(Debug, Clone)]
pub struct Spec {
    pub fields: Vec<Field>,
}

/// A single named command with input/output schemas.
#[derive(Debug, Clone)]
pub struct Command {
    pub name: String,
    pub description: String,
    pub input: StructDef,
    pub output: StructDef,
}

/// A topic group of hooks (e.g. "forest/deployment").
#[derive(Debug, Clone)]
pub struct HookGroup {
    pub topic: String,
    pub actions: Vec<HookAction>,
}

/// One action within a hook group.
#[derive(Debug, Clone)]
pub struct HookAction {
    pub name: String,
    pub description: String,
    pub input: StructDef,
    pub output: Option<StructDef>,
}

/// A named type definition.
#[derive(Debug, Clone)]
pub struct TypeDef {
    pub name: String,
    pub kind: TypeDefKind,
}

#[derive(Debug, Clone)]
pub enum TypeDefKind {
    Struct(StructDef),
    Enum(EnumDef),
}

/// An object with named fields.
#[derive(Debug, Clone)]
pub struct StructDef {
    pub fields: Vec<Field>,
}

/// A single typed field.
#[derive(Debug, Clone)]
pub struct Field {
    pub name: String,
    pub ty: TypeRef,
    pub required: bool,
    pub default_value: Option<DefaultValue>,
    pub description: Option<String>,
    pub constraints: Vec<Constraint>,
}

/// A reference to a type in the IR.
#[derive(Debug, Clone)]
pub enum TypeRef {
    String,
    Integer,
    Float,
    Boolean,
    /// Reference to a named type (TypeDef).
    Named(std::string::String),
    /// Array of inner type.
    Array(Box<TypeRef>),
    /// Inline anonymous object.
    Object(StructDef),
    /// Map type (string keys → value type).
    Map(Box<TypeRef>),
}

/// Enumeration: either string-valued or integer-valued.
#[derive(Debug, Clone)]
pub enum EnumDef {
    StringEnum(Vec<std::string::String>),
    IntegerEnum(Vec<i64>),
}

#[derive(Debug, Clone)]
pub enum DefaultValue {
    String(std::string::String),
    Integer(i64),
    Float(f64),
    Boolean(bool),
    Object(serde_json::Value),
    Null,
}

#[derive(Debug, Clone)]
pub enum Constraint {
    Minimum(i64),
    Maximum(i64),
    ExclusiveMinimum(i64),
    ExclusiveMaximum(i64),
    Pattern(std::string::String),
    MinLength(u64),
    MaxLength(u64),
    MinItems(u64),
    MaxItems(u64),
    UniqueItems,
}
