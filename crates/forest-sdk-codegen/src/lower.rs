use std::cell::RefCell;
use std::collections::BTreeMap;

use crate::errors::{CodegenResult, Error};
use crate::ir;
use crate::openapi::models::{Document, Reference, Schema, SchemaOrRef};

pub fn lower(doc: &Document) -> CodegenResult<ir::Module> {
    let ctx = LoweringContext::new(doc)?;
    ctx.lower()
}

#[derive(Debug, Clone, PartialEq)]
enum SchemaClass {
    Spec,
    Commands,
    Hooks,
    ForestBase,
    UserType,
}

struct LoweringContext<'a> {
    schemas: &'a BTreeMap<String, SchemaOrRef>,
    classifications: BTreeMap<String, SchemaClass>,
    /// Inline enums promoted to named types during lowering.
    promoted_type_defs: RefCell<Vec<ir::TypeDef>>,
}

impl<'a> LoweringContext<'a> {
    fn new(doc: &'a Document) -> CodegenResult<Self> {
        let schemas = match &doc.components {
            Some(c) => &c.schemas,
            None => {
                return Err(Error::LoweringError(
                    "document has no components.schemas".into(),
                ));
            }
        };

        let mut classifications = BTreeMap::new();
        for (name, schema_or_ref) in schemas {
            let class = Self::classify(name, schema_or_ref, schemas);
            classifications.insert(name.clone(), class);
        }

        Ok(Self {
            schemas,
            classifications,
            promoted_type_defs: RefCell::new(Vec::new()),
        })
    }

    fn classify(
        name: &str,
        schema_or_ref: &SchemaOrRef,
        _schemas: &BTreeMap<String, SchemaOrRef>,
    ) -> SchemaClass {
        // Forest base types: names starting with "Forest" or containing "forest."
        if name.starts_with("Forest") || name.contains("forest.") {
            return SchemaClass::ForestBase;
        }

        let schema = match schema_or_ref {
            SchemaOrRef::Schema(s) => s,
            SchemaOrRef::Ref(_) => return SchemaClass::UserType,
        };

        if Self::has_allof_ref_matching(schema, "Spec") {
            SchemaClass::Spec
        } else if Self::has_allof_ref_matching(schema, "Commands") {
            SchemaClass::Commands
        } else if Self::has_allof_ref_matching(schema, "Hooks") {
            SchemaClass::Hooks
        } else {
            SchemaClass::UserType
        }
    }

    /// Check if a schema's allOf contains a $ref whose target name ends with
    /// the given suffix. This matches both `ForestSpec` and `forest.Spec` patterns.
    fn has_allof_ref_matching(schema: &Schema, suffix: &str) -> bool {
        let Some(all_of) = &schema.all_of else {
            return false;
        };
        for item in all_of {
            if let SchemaOrRef::Ref(Reference { ref_path }) = item {
                let ref_name = ref_path
                    .strip_prefix("#/components/schemas/")
                    .unwrap_or(ref_path);
                // Match "ForestSpec", "forest.Spec", etc.
                if ref_name.ends_with(suffix)
                    && (ref_name.starts_with("Forest") || ref_name.contains("forest."))
                {
                    return true;
                }
            }
        }
        false
    }

    // ── $ref resolution ──────────────────────────────────────────────

    fn resolve_ref<'b>(&'b self, ref_path: &str) -> CodegenResult<&'a Schema> {
        let path = ref_path
            .strip_prefix("#/components/schemas/")
            .ok_or_else(|| Error::LoweringError(format!("unsupported $ref path: {ref_path}")))?;

        // Dot-notation: "SchemaName.propertyName"
        if let Some((schema_name, prop_name)) = path.split_once('.') {
            let parent = self.get_schema(schema_name)?;
            let properties = parent.properties.as_ref().ok_or_else(|| {
                Error::LoweringError(format!(
                    "schema {schema_name} has no properties for dot-ref {ref_path}"
                ))
            })?;
            match properties.get(prop_name) {
                Some(SchemaOrRef::Schema(s)) => Ok(s),
                Some(SchemaOrRef::Ref(r)) => self.resolve_ref(&r.ref_path),
                None => Err(Error::LoweringError(format!(
                    "property {prop_name} not found in {schema_name}"
                ))),
            }
        } else {
            self.get_schema(path)
        }
    }

    fn get_schema(&self, name: &str) -> CodegenResult<&'a Schema> {
        match self.schemas.get(name) {
            Some(SchemaOrRef::Schema(s)) => Ok(s),
            Some(SchemaOrRef::Ref(r)) => self.resolve_ref(&r.ref_path),
            None => Err(Error::LoweringError(format!("schema not found: {name}"))),
        }
    }

    fn ref_to_type_name(&self, ref_path: &str) -> CodegenResult<String> {
        let path = ref_path
            .strip_prefix("#/components/schemas/")
            .ok_or_else(|| Error::LoweringError(format!("unsupported $ref path: {ref_path}")))?;
        // For dot-notation refs, we don't produce a named type reference
        if path.contains('.') {
            return Err(Error::LoweringError(format!(
                "dot-notation ref cannot be used as type name: {ref_path}"
            )));
        }
        Ok(path.to_string())
    }

    // ── allOf helpers ────────────────────────────────────────────────

    fn collect_required_fields(&self, schema: &Schema) -> Vec<String> {
        let mut required: Vec<String> = schema.required.clone().unwrap_or_default();
        if let Some(all_of) = &schema.all_of {
            for item in all_of {
                if let SchemaOrRef::Schema(s) = item
                    && let Some(req) = &s.required
                {
                    for r in req {
                        if !required.contains(r) {
                            required.push(r.clone());
                        }
                    }
                }
            }
        }
        required
    }

    // ── Type lowering ────────────────────────────────────────────────

    fn lower_type_ref(&self, schema_or_ref: &SchemaOrRef) -> CodegenResult<ir::TypeRef> {
        match schema_or_ref {
            SchemaOrRef::Ref(r) => {
                // Dot-notation refs: resolve inline
                if r.ref_path.contains('.') {
                    let resolved = self.resolve_ref(&r.ref_path)?;
                    self.lower_schema_type_ref(resolved)
                } else {
                    let name = self.ref_to_type_name(&r.ref_path)?;
                    Ok(ir::TypeRef::Named(name))
                }
            }
            SchemaOrRef::Schema(s) => self.lower_schema_type_ref(s),
        }
    }

    fn lower_schema_type_ref(&self, schema: &Schema) -> CodegenResult<ir::TypeRef> {
        // Handle oneOf by preferring a $ref branch
        if let Some(one_of) = &schema.one_of {
            for variant in one_of {
                if let SchemaOrRef::Ref(r) = variant
                    && !r.ref_path.contains('.')
                {
                    let name = self.ref_to_type_name(&r.ref_path)?;
                    return Ok(ir::TypeRef::Named(name));
                }
            }
        }

        match schema.schema_type.as_deref() {
            Some("string") => Ok(ir::TypeRef::String),
            Some("integer") => Ok(ir::TypeRef::Integer),
            Some("number") => Ok(ir::TypeRef::Float),
            Some("boolean") => Ok(ir::TypeRef::Boolean),
            Some("array") => {
                let items = schema
                    .items
                    .as_ref()
                    .ok_or_else(|| Error::LoweringError("array without items".into()))?;
                let inner = self.lower_type_ref(items)?;
                Ok(ir::TypeRef::Array(Box::new(inner)))
            }
            Some("object") | None => {
                if let Some(additional) = &schema.additional_properties {
                    let inner = self.lower_type_ref(additional)?;
                    Ok(ir::TypeRef::Map(Box::new(inner)))
                } else if let Some(props) = &schema.properties {
                    let required = self.collect_required_fields(schema);
                    let fields = self.lower_properties(props, &required)?;
                    Ok(ir::TypeRef::Object(ir::StructDef { fields }))
                } else {
                    Ok(ir::TypeRef::Object(ir::StructDef { fields: vec![] }))
                }
            }
            Some(other) => Err(Error::LoweringError(format!(
                "unsupported schema type: {other}"
            ))),
        }
    }

    fn lower_properties(
        &self,
        props: &BTreeMap<String, SchemaOrRef>,
        required: &[String],
    ) -> CodegenResult<Vec<ir::Field>> {
        let mut fields = Vec::new();
        for (name, schema_or_ref) in props {
            let field = self.lower_field(name, schema_or_ref, required.contains(name))?;
            fields.push(field);
        }
        Ok(fields)
    }

    fn lower_field(
        &self,
        name: &str,
        schema_or_ref: &SchemaOrRef,
        required: bool,
    ) -> CodegenResult<ir::Field> {
        let (ty, description, default_value, constraints) = match schema_or_ref {
            SchemaOrRef::Ref(r) => {
                let ty = if r.ref_path.contains('.') {
                    let resolved = self.resolve_ref(&r.ref_path)?;
                    self.lower_schema_type_ref(resolved)?
                } else {
                    let type_name = self.ref_to_type_name(&r.ref_path)?;
                    ir::TypeRef::Named(type_name)
                };
                (ty, None, None, vec![])
            }
            SchemaOrRef::Schema(s) => {
                let ty = self.lower_field_type(name, s)?;
                let description = s.description.clone();
                let default_value = s.default.as_ref().map(lower_default_value);
                let constraints = lower_constraints(s);
                (ty, description, default_value, constraints)
            }
        };

        Ok(ir::Field {
            name: name.to_string(),
            ty,
            required,
            default_value,
            description,
            constraints,
        })
    }

    /// Lower a field's type, handling inline enums specially.
    /// Multi-value inline enums are matched against existing types or promoted
    /// to new named types.
    fn lower_field_type(&self, field_name: &str, schema: &Schema) -> CodegenResult<ir::TypeRef> {
        if let Some(enum_vals) = &schema.enum_values
            && enum_vals.len() > 1
        {
            // Try to match an existing user type with the same enum values
            if let Some(type_name) = self.find_matching_enum_type(schema, enum_vals) {
                return Ok(ir::TypeRef::Named(type_name));
            }
            // Promote to a new named type
            let promoted_name = to_pascal_case(field_name);
            let enum_def = lower_enum_values(schema, enum_vals)?;
            self.promoted_type_defs.borrow_mut().push(ir::TypeDef {
                name: promoted_name.clone(),
                kind: ir::TypeDefKind::Enum(enum_def),
            });
            return Ok(ir::TypeRef::Named(promoted_name));
        }

        self.lower_schema_type_ref(schema)
    }

    /// Try to find a user-defined enum type with matching values.
    fn find_matching_enum_type(
        &self,
        schema: &Schema,
        enum_vals: &[serde_json::Value],
    ) -> Option<String> {
        for (name, class) in &self.classifications {
            if *class != SchemaClass::UserType {
                continue;
            }
            let Ok(candidate) = self.get_schema(name) else {
                continue;
            };
            if let Some(candidate_vals) = &candidate.enum_values
                && candidate.schema_type == schema.schema_type
                && candidate_vals == enum_vals
            {
                return Some(name.clone());
            }
        }
        None
    }

    // ── Top-level lowering ───────────────────────────────────────────

    fn lower(&self) -> CodegenResult<ir::Module> {
        let mut spec = None;
        let mut commands = Vec::new();
        let mut hook_groups = Vec::new();
        let mut type_defs = Vec::new();

        for (name, class) in &self.classifications {
            let schema = self.get_schema(name)?;
            match class {
                SchemaClass::Spec => {
                    spec = Some(self.lower_spec(schema)?);
                }
                SchemaClass::Commands => {
                    commands = self.lower_commands(schema)?;
                }
                SchemaClass::Hooks => {
                    hook_groups = self.lower_hooks(schema)?;
                }
                SchemaClass::UserType => {
                    type_defs.push(self.lower_user_type(name, schema)?);
                }
                SchemaClass::ForestBase => {}
            }
        }

        // Append any inline enums that were promoted to named types
        type_defs.extend(self.promoted_type_defs.borrow_mut().drain(..));

        Ok(ir::Module {
            spec: spec.ok_or_else(|| Error::LoweringError("no Spec schema found".into()))?,
            commands,
            hook_groups,
            type_defs,
        })
    }

    // ── Component ────────────────────────────────────────────────────

    #[allow(dead_code)]
    fn lower_component(&self, schema: &Schema) -> CodegenResult<ir::Component> {
        let props = schema
            .properties
            .as_ref()
            .ok_or_else(|| Error::LoweringError("Component schema has no properties".into()))?;

        let name = self.extract_single_enum_string(props, "name")?;
        let org = self.extract_single_enum_string(props, "org")?;
        let version = self.extract_single_enum_string(props, "version")?;

        Ok(ir::Component { name, org, version })
    }

    #[allow(dead_code)]
    fn extract_single_enum_string(
        &self,
        props: &BTreeMap<String, SchemaOrRef>,
        field: &str,
    ) -> CodegenResult<String> {
        let schema = match props.get(field) {
            Some(SchemaOrRef::Schema(s)) => s,
            Some(SchemaOrRef::Ref(r)) => self.resolve_ref(&r.ref_path)?,
            None => {
                return Err(Error::LoweringError(format!("missing property: {field}")));
            }
        };
        match &schema.enum_values {
            Some(vals) if vals.len() == 1 => {
                vals[0].as_str().map(|s| s.to_string()).ok_or_else(|| {
                    Error::LoweringError(format!("property {field} enum[0] is not a string"))
                })
            }
            _ => Err(Error::LoweringError(format!(
                "property {field} is not a single-value string enum"
            ))),
        }
    }

    // ── Spec ─────────────────────────────────────────────────────────

    fn lower_spec(&self, schema: &Schema) -> CodegenResult<ir::Spec> {
        let props = schema
            .properties
            .as_ref()
            .ok_or_else(|| Error::LoweringError("Spec schema has no properties".into()))?;
        let required = self.collect_required_fields(schema);

        let mut fields = Vec::new();
        for (name, schema_or_ref) in props {
            let field = self.lower_field(name, schema_or_ref, required.contains(name))?;
            fields.push(field);
        }

        Ok(ir::Spec { fields })
    }

    // ── Commands ─────────────────────────────────────────────────────

    fn lower_commands(&self, schema: &Schema) -> CodegenResult<Vec<ir::Command>> {
        let props = schema
            .properties
            .as_ref()
            .ok_or_else(|| Error::LoweringError("Commands schema has no properties".into()))?;

        let mut commands = Vec::new();
        for (cmd_name, cmd_sor) in props {
            let cmd_schema = match cmd_sor {
                SchemaOrRef::Schema(s) => s,
                SchemaOrRef::Ref(r) => self.resolve_ref(&r.ref_path)?,
            };

            let description = self.extract_description(cmd_schema)?;
            let input = self.extract_io_struct(cmd_schema, "input")?;
            let output = self.extract_io_struct(cmd_schema, "output")?;

            commands.push(ir::Command {
                name: cmd_name.clone(),
                description,
                input,
                output,
            });
        }
        Ok(commands)
    }

    // ── Hooks ────────────────────────────────────────────────────────

    fn lower_hooks(&self, schema: &Schema) -> CodegenResult<Vec<ir::HookGroup>> {
        let props = schema
            .properties
            .as_ref()
            .ok_or_else(|| Error::LoweringError("Hooks schema has no properties".into()))?;

        let mut groups = Vec::new();
        for (topic, topic_sor) in props {
            let topic_schema = match topic_sor {
                SchemaOrRef::Schema(s) => s,
                SchemaOrRef::Ref(r) => self.resolve_ref(&r.ref_path)?,
            };

            let actions = self.lower_hook_actions(topic_schema)?;
            groups.push(ir::HookGroup {
                topic: topic.clone(),
                actions,
            });
        }
        Ok(groups)
    }

    fn lower_hook_actions(&self, topic_schema: &Schema) -> CodegenResult<Vec<ir::HookAction>> {
        // Collect action schemas from allOf refs first (base contract),
        // then overlay inline properties (user overrides like description).
        let mut merged_actions: BTreeMap<String, &Schema> = BTreeMap::new();

        // 1. Collect from allOf refs (e.g. DeploymentHooks contract)
        if let Some(all_of) = &topic_schema.all_of {
            for item in all_of {
                if let SchemaOrRef::Ref(r) = item {
                    if let Ok(ref_schema) = self.resolve_ref(&r.ref_path) {
                        if let Some(ref_props) = &ref_schema.properties {
                            for (name, sor) in ref_props {
                                let schema = match sor {
                                    SchemaOrRef::Schema(s) => s,
                                    SchemaOrRef::Ref(r2) => self.resolve_ref(&r2.ref_path)?,
                                };
                                merged_actions.insert(name.clone(), schema);
                            }
                        }
                    }
                }
            }
        }

        // 2. Inline properties override/extend base actions
        let inline_props = topic_schema.properties.as_ref();

        let mut actions = Vec::new();
        // Process all known action names (from base + inline)
        let action_names: Vec<String> = {
            let mut names: Vec<String> = merged_actions.keys().cloned().collect();
            if let Some(props) = inline_props {
                for name in props.keys() {
                    if !names.contains(name) {
                        names.push(name.clone());
                    }
                }
            }
            names
        };

        for action_name in &action_names {
            let base_schema = merged_actions.get(action_name);
            let inline_schema = inline_props.and_then(|p| p.get(action_name));

            // Use inline for description (user override), base for input/output
            let desc_schema: Option<&Schema> = match inline_schema {
                Some(SchemaOrRef::Schema(s)) => Some(s.as_ref()),
                Some(SchemaOrRef::Ref(r)) => Some(self.resolve_ref(&r.ref_path)?),
                None => base_schema.copied(),
            };
            let io_schema: &Schema = match base_schema {
                Some(s) => s,
                None => match inline_schema {
                    Some(SchemaOrRef::Schema(s)) => s.as_ref(),
                    Some(SchemaOrRef::Ref(r)) => self.resolve_ref(&r.ref_path)?,
                    None => {
                        return Err(Error::LoweringError(format!(
                            "hook action {action_name} has no schema"
                        )));
                    }
                },
            };

            let description = match desc_schema {
                Some(s) => self.extract_description(s)?,
                None => String::new(),
            };
            let input = self.extract_io_struct(io_schema, "input")?;
            let output = if io_schema
                .properties
                .as_ref()
                .is_some_and(|p| p.contains_key("output"))
            {
                Some(self.extract_io_struct(io_schema, "output")?)
            } else {
                None
            };

            actions.push(ir::HookAction {
                name: action_name.clone(),
                description,
                input,
                output,
            });
        }
        Ok(actions)
    }

    // ── User types ───────────────────────────────────────────────────

    fn lower_user_type(&self, name: &str, schema: &Schema) -> CodegenResult<ir::TypeDef> {
        // Top-level enum type
        if let Some(enum_vals) = &schema.enum_values {
            let enum_def = lower_enum_values(schema, enum_vals)?;
            return Ok(ir::TypeDef {
                name: name.to_string(),
                kind: ir::TypeDefKind::Enum(enum_def),
            });
        }

        // Map type (object with additionalProperties, no named properties)
        if schema.properties.is_none() {
            if let Some(additional) = &schema.additional_properties {
                let inner = self.lower_type_ref(additional)?;
                return Ok(ir::TypeDef {
                    name: name.to_string(),
                    kind: ir::TypeDefKind::Map(inner),
                });
            }
        }

        // Struct type
        let required = self.collect_required_fields(schema);
        let fields = if let Some(props) = &schema.properties {
            self.lower_properties(props, &required)?
        } else {
            vec![]
        };

        Ok(ir::TypeDef {
            name: name.to_string(),
            kind: ir::TypeDefKind::Struct(ir::StructDef { fields }),
        })
    }

    // ── Shared helpers ───────────────────────────────────────────────

    fn extract_description(&self, schema: &Schema) -> CodegenResult<String> {
        let Some(props) = &schema.properties else {
            return Ok(String::new());
        };
        let desc_schema = match props.get("description") {
            Some(SchemaOrRef::Schema(s)) => s,
            Some(SchemaOrRef::Ref(r)) => self.resolve_ref(&r.ref_path)?,
            None => return Ok(String::new()),
        };
        match &desc_schema.enum_values {
            Some(vals) if !vals.is_empty() => vals[0]
                .as_str()
                .map(|s| s.to_string())
                .ok_or_else(|| Error::LoweringError("description enum[0] is not a string".into())),
            _ => Ok(desc_schema.description.clone().unwrap_or_default()),
        }
    }

    fn extract_io_struct(&self, schema: &Schema, field_name: &str) -> CodegenResult<ir::StructDef> {
        let Some(props) = &schema.properties else {
            return Ok(ir::StructDef { fields: vec![] });
        };
        let io_schema = match props.get(field_name) {
            Some(SchemaOrRef::Schema(s)) => s,
            Some(SchemaOrRef::Ref(r)) => self.resolve_ref(&r.ref_path)?,
            None => return Ok(ir::StructDef { fields: vec![] }),
        };

        if let Some(obj_props) = &io_schema.properties {
            let required = io_schema.required.clone().unwrap_or_default();
            let fields = self.lower_properties(obj_props, &required)?;
            Ok(ir::StructDef { fields })
        } else {
            Ok(ir::StructDef { fields: vec![] })
        }
    }
}

// ── Free functions ───────────────────────────────────────────────────

fn lower_enum_values(schema: &Schema, vals: &[serde_json::Value]) -> CodegenResult<ir::EnumDef> {
    match schema.schema_type.as_deref() {
        Some("string") => {
            let variants: Result<Vec<String>, _> = vals
                .iter()
                .map(|v| {
                    v.as_str().map(|s| s.to_string()).ok_or_else(|| {
                        Error::LoweringError("string enum contains non-string value".into())
                    })
                })
                .collect();
            Ok(ir::EnumDef::StringEnum(variants?))
        }
        Some("integer") => {
            let variants: Result<Vec<i64>, _> = vals
                .iter()
                .map(|v| {
                    v.as_i64().ok_or_else(|| {
                        Error::LoweringError("integer enum contains non-integer value".into())
                    })
                })
                .collect();
            Ok(ir::EnumDef::IntegerEnum(variants?))
        }
        other => Err(Error::LoweringError(format!(
            "enum with unsupported type: {other:?}"
        ))),
    }
}

fn lower_default_value(v: &serde_json::Value) -> ir::DefaultValue {
    match v {
        serde_json::Value::String(s) => ir::DefaultValue::String(s.clone()),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                ir::DefaultValue::Integer(i)
            } else if let Some(f) = n.as_f64() {
                ir::DefaultValue::Float(f)
            } else {
                ir::DefaultValue::Object(v.clone())
            }
        }
        serde_json::Value::Bool(b) => ir::DefaultValue::Boolean(*b),
        serde_json::Value::Null => ir::DefaultValue::Null,
        _ => ir::DefaultValue::Object(v.clone()),
    }
}

fn lower_constraints(schema: &Schema) -> Vec<ir::Constraint> {
    let mut constraints = Vec::new();

    if let Some(n) = &schema.minimum
        && let Some(i) = n.as_i64()
    {
        constraints.push(ir::Constraint::Minimum(i));
    }
    if let Some(n) = &schema.maximum
        && let Some(i) = n.as_i64()
    {
        constraints.push(ir::Constraint::Maximum(i));
    }
    if let Some(v) = &schema.exclusive_minimum {
        if let Some(true) = v.as_bool() {
            // OpenAPI 3.0 boolean style — shift minimum by 1
            if let Some(n) = &schema.minimum
                && let Some(i) = n.as_i64()
            {
                // Replace Minimum with ExclusiveMinimum
                constraints.retain(|c| !matches!(c, ir::Constraint::Minimum(_)));
                constraints.push(ir::Constraint::ExclusiveMinimum(i));
            }
        } else if let Some(i) = v.as_i64() {
            // OpenAPI 3.1 number style
            constraints.push(ir::Constraint::ExclusiveMinimum(i));
        }
    }
    if let Some(v) = &schema.exclusive_maximum {
        if let Some(true) = v.as_bool() {
            if let Some(n) = &schema.maximum
                && let Some(i) = n.as_i64()
            {
                constraints.retain(|c| !matches!(c, ir::Constraint::Maximum(_)));
                constraints.push(ir::Constraint::ExclusiveMaximum(i));
            }
        } else if let Some(i) = v.as_i64() {
            constraints.push(ir::Constraint::ExclusiveMaximum(i));
        }
    }
    if let Some(p) = &schema.pattern {
        constraints.push(ir::Constraint::Pattern(p.clone()));
    }
    if let Some(n) = schema.min_length {
        constraints.push(ir::Constraint::MinLength(n));
    }
    if let Some(n) = schema.max_length {
        constraints.push(ir::Constraint::MaxLength(n));
    }
    if let Some(n) = schema.min_items {
        constraints.push(ir::Constraint::MinItems(n));
    }
    if let Some(n) = schema.max_items {
        constraints.push(ir::Constraint::MaxItems(n));
    }
    if schema.unique_items == Some(true) {
        constraints.push(ir::Constraint::UniqueItems);
    }

    constraints
}

pub fn to_pascal_case(s: &str) -> String {
    s.split(['_', '-', '/'])
        .filter(|seg| !seg.is_empty())
        .map(|seg| {
            let mut chars = seg.chars();
            let first = chars
                .next()
                .map(|c| c.to_uppercase().to_string())
                .unwrap_or_default();
            format!("{first}{}", chars.as_str())
        })
        .collect()
}
