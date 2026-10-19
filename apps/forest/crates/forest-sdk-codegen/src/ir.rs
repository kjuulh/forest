#[derive(Debug, Clone)]
pub struct Module {
    pub spec: Spec,
    pub commands: Vec<Command>,
    pub hook_groups: Vec<HookGroup>,
    pub type_defs: Vec<TypeDef>,
}

#[derive(Debug, Clone)]
pub struct Component {
    pub name: String,
    pub org: String,
    pub version: String,
}

#[derive(Debug, Clone)]
pub struct Spec {
    pub fields: Vec<Field>,
}

#[derive(Debug, Clone)]
pub struct Command {
    pub name: String,
    pub description: String,
    pub input: StructDef,
    pub output: StructDef,
}

#[derive(Debug, Clone)]
pub struct HookGroup {
    pub topic: String,
    pub actions: Vec<HookAction>,
}

#[derive(Debug, Clone)]
pub struct HookAction {
    pub name: String,
    pub description: String,
    pub input: StructDef,
    pub output: Option<StructDef>,
}

#[derive(Debug, Clone)]
pub struct TypeDef {
    pub name: String,
    pub kind: TypeDefKind,
}

#[derive(Debug, Clone)]
pub enum TypeDefKind {
    Struct(StructDef),
    Enum(EnumDef),
    Map(TypeRef),
}

#[derive(Debug, Clone)]
pub struct StructDef {
    pub fields: Vec<Field>,
}

#[derive(Debug, Clone)]
pub struct Field {
    pub name: String,
    pub ty: TypeRef,
    pub required: bool,
    pub default_value: Option<DefaultValue>,
    pub description: Option<String>,
    pub constraints: Vec<Constraint>,
}

#[derive(Debug, Clone)]
pub enum TypeRef {
    String,
    Integer,
    Float,
    Boolean,
    Named(std::string::String),
    Array(Box<TypeRef>),
    Object(StructDef),
    Map(Box<TypeRef>),
}

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
