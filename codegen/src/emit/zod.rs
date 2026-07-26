//! Zod emitter — renders [`ir::Schema`] into Zod schema source (TypeScript).
//!
//! Zod schemas are runtime validators. The generated `export const` values
//! mirror the data dialect's `struct` / `enum`, the entity dialect's
//! `record` / `relation`, and the protocol dialect's request / response /
//! event payloads.
//!
//! ## Tier 1 type mapping
//!
//! - `link<Record>` → `z.string()` (the linked record's id).
//! - `'literal'` → `z.literal("value")`.
//! - `A | B` → `z.union([...])`; a union of string literals collapses to a
//!   `z.enum([...])` (Zod's idiomatic closed-string-set validator).
//! - `record` → a `z.object({...})` with a leading `id: z.string()`.
//! - `relation` → a `z.object({...})` with `id` / `in` / `out: z.string()`.
//!
//! ## Ordering
//!
//! Unlike the TypeScript emitter (whose `interface` declarations are types and
//! thus order-independent), a Zod schema is a **value** — `z.object({ role:
//! Role })` needs `Role` defined first. The emitter therefore writes all
//! `enum` schemas before any `object` schema. Struct-to-struct references rely
//! on source order (a forward reference between two structs is uncommon and
//! out of Phase 1 scope).

use crate::Emitter;
use crate::ir;

use super::case::to_pascal_case;

/// The Zod code generation target.
#[derive(Debug, Default, Clone, Copy)]
pub struct ZodEmitter;

impl ZodEmitter {
    /// Create a new [`ZodEmitter`].
    pub fn new() -> Self {
        Self
    }
}

impl Emitter for ZodEmitter {
    fn emit(&self, schema: &ir::Schema) -> String {
        let mut code = String::new();
        code.push_str(HEADER);
        code.push('\n');

        // enums first — a Zod schema is a value and cannot be forward-referenced.
        for ty in &schema.types {
            if let ir::TypeDef::Enum {
                name,
                description,
                variants,
            } = ty
            {
                code.push_str(&render_enum(name, description.as_deref(), variants));
                code.push_str("\n\n");
            }
        }
        // then structs.
        for ty in &schema.types {
            if let ir::TypeDef::Struct {
                name,
                description,
                fields,
            } = ty
            {
                code.push_str(&render_object(name, description.as_deref(), fields));
                code.push_str("\n\n");
            }
        }

        // entity dialect — records and relations.
        for record in &schema.records {
            code.push_str(&render_object(
                &record.name,
                record.description.as_deref(),
                &record_members(record),
            ));
            code.push_str("\n\n");
        }
        for relation in &schema.relations {
            code.push_str(&render_object(
                &relation.name,
                relation.description.as_deref(),
                &relation_members(relation),
            ));
            code.push_str("\n\n");
        }

        // protocol dialect — request / response / event payload schemas.
        if let Some(protocol) = &schema.protocol {
            for channel in &protocol.channels {
                for req in &channel.requests {
                    code.push_str(&render_object(&req.name, None, &req.fields));
                    code.push_str("\n\n");
                    if let Some(returns) = &req.returns {
                        code.push_str(&render_object(&returns.name, None, &returns.fields));
                        code.push_str("\n\n");
                    }
                }
                for evt in &channel.events {
                    code.push_str(&render_object(&evt.name, None, &evt.fields));
                    code.push_str("\n\n");
                }
                // discriminated-union envelopes, emitted after the per-payload
                // objects they reference: one over the channel's requests, one
                // over its events (a channel can carry both directions, and the
                // two sets are dispatched by different peers).
                if let Some(tag) = &channel.envelope {
                    let request_names: Vec<&str> =
                        channel.requests.iter().map(|r| r.name.as_str()).collect();
                    if !request_names.is_empty() {
                        code.push_str(&render_envelope(
                            tag,
                            &format!("{}Envelope", to_pascal_case(&channel.name)),
                            &request_names,
                        ));
                        code.push_str("\n\n");
                    }
                    let event_names: Vec<&str> =
                        channel.events.iter().map(|e| e.name.as_str()).collect();
                    if !event_names.is_empty() {
                        code.push_str(&render_envelope(
                            tag,
                            &format!("{}EventEnvelope", to_pascal_case(&channel.name)),
                            &event_names,
                        ));
                        code.push_str("\n\n");
                    }
                }
            }
        }

        code
    }
}

/// Fixed header block.
const HEADER: &str = "\
// Auto-generated Zod schemas
// DO NOT EDIT MANUALLY
import { z } from \"zod\";
";

/// Render a JavaScript string literal (double-quoted, with `"` and `\`
/// escaped) for use inside a generated Zod call such as `.describe(...)`.
fn js_string(text: &str) -> String {
    let escaped = text.replace('\\', "\\\\").replace('"', "\\\"");
    format!("\"{escaped}\"")
}

/// Append a `.describe("...")` call when the type definition carries a
/// description.
fn describe_suffix(description: Option<&str>) -> String {
    match description {
        Some(text) => format!(".describe({})", js_string(text)),
        None => String::new(),
    }
}

/// Render an `enum` as a `z.enum([...])`.
fn render_enum(name: &str, description: Option<&str>, variants: &[String]) -> String {
    let vs: Vec<String> = variants.iter().map(|v| format!("\"{v}\"")).collect();
    format!(
        "export const {} = z.enum([{}]){};",
        to_pascal_case(name),
        vs.join(", "),
        describe_suffix(description),
    )
}

/// Render a `struct` / payload as a `z.object({...})`.
fn render_object(name: &str, description: Option<&str>, fields: &[ir::Field]) -> String {
    let pascal = to_pascal_case(name);
    let describe = describe_suffix(description);
    if fields.is_empty() {
        return format!("export const {pascal} = z.object({{}}){describe};");
    }
    let body: Vec<String> = fields.iter().map(render_field).collect();
    format!(
        "export const {pascal} = z.object({{\n{}\n}}){describe};",
        body.join("\n")
    )
}

/// Render the channel's envelope as a `z.discriminatedUnion`.
///
/// Each member extends a per-request `z.object` with the discriminator
/// (`tag`) literal, so a runtime value carrying `{ "<tag>": "<wire>" }`
/// validates against exactly one request payload. A fieldless request's
/// object is `z.object({})`, which `.extend` still accepts.
fn render_envelope(tag: &str, name: &str, members: &[&str]) -> String {
    let mut out = format!(
        "export const {name} = z.discriminatedUnion({}, [\n",
        js_string(tag)
    );
    for member in members {
        out.push_str(&format!(
            "  {}.extend({{ {tag}: z.literal({}) }}),\n",
            to_pascal_case(member),
            js_string(member),
        ));
    }
    out.push_str("]);");
    out
}

/// Render the constraint suffix for a Zod schema: `.min(N)` / `.max(N)` for a
/// numeric range, `.min(N)` / `.max(N)` for a string / array length bound, and
/// `.regex(/.../)` for a pattern. Numeric range and length bound never both
/// apply to one field (a field is either numeric or string / array), so the
/// shared `.min` / `.max` call sites do not collide.
fn constraint_suffix(c: &ir::Constraints) -> String {
    let mut out = String::new();
    if let Some(min) = c.min.or(c.min_length.map(|n| n as i64)) {
        out.push_str(&format!(".min({min})"));
    }
    if let Some(max) = c.max.or(c.max_length.map(|n| n as i64)) {
        out.push_str(&format!(".max({max})"));
    }
    if let Some(pattern) = &c.pattern {
        out.push_str(&format!(".regex(/{pattern}/)"));
    }
    out
}

/// Render a single object field line, applying any `.describe(...)` and
/// constraint suffixes before the `.optional()` wrapper.
fn render_field(field: &ir::Field) -> String {
    let mut schema = ty_to_zod(&field.ty);
    schema.push_str(&constraint_suffix(&field.constraints));
    if let Some(desc) = &field.description {
        schema.push_str(&format!(".describe({})", js_string(desc)));
    }
    if !field.required {
        schema.push_str(".optional()");
    }
    format!("  {}: {},", field.name, schema)
}

/// The synthetic `id: z.string()` member shared by records and relations.
fn id_member() -> ir::Field {
    ir::Field {
        name: "id".to_string(),
        ty: ir::Ty::Primitive(ir::Prim::String),
        required: true,
        flexible: false,
        default: None,
        description: None,
        constraints: ir::Constraints::default(),
    }
}

/// A record's object members: a leading `id`, then its declared fields.
fn record_members(record: &ir::Record) -> Vec<ir::Field> {
    let mut members = Vec::with_capacity(record.fields.len() + 1);
    members.push(id_member());
    members.extend(record.fields.iter().cloned());
    members
}

/// A relation's edge-object members: `id` / `in` / `out`, then its declared
/// edge-property fields.
fn relation_members(relation: &ir::Relation) -> Vec<ir::Field> {
    let endpoint = |name: &str| ir::Field {
        name: name.to_string(),
        ty: ir::Ty::Primitive(ir::Prim::String),
        required: true,
        flexible: false,
        default: None,
        description: None,
        constraints: ir::Constraints::default(),
    };
    let mut members = Vec::with_capacity(relation.fields.len() + 3);
    members.push(id_member());
    members.push(endpoint("in"));
    members.push(endpoint("out"));
    members.extend(relation.fields.iter().cloned());
    members
}

/// Map an [`ir::Ty`] to its Zod schema expression.
fn ty_to_zod(ty: &ir::Ty) -> String {
    match ty {
        ir::Ty::Primitive(p) => prim_to_zod(*p).to_string(),
        ir::Ty::Array(inner) => format!("z.array({})", ty_to_zod(inner)),
        // a named type references another generated schema by identifier.
        ir::Ty::Named(name) => to_pascal_case(name),
        // a link is validated as the target record's id — a string.
        ir::Ty::Link(_) => "z.string()".to_string(),
        // a string literal → `z.literal(...)`.
        ir::Ty::Literal(value) => format!("z.literal(\"{value}\")"),
        ir::Ty::Union(members) => {
            // A union of string literals → the idiomatic `z.enum([...])`.
            if let Some(values) = literal_union_values(members) {
                let vs: Vec<String> = values.iter().map(|v| format!("\"{v}\"")).collect();
                format!("z.enum([{}])", vs.join(", "))
            } else {
                let mut parts: Vec<String> = Vec::new();
                for m in members {
                    let p = ty_to_zod(m);
                    if !parts.contains(&p) {
                        parts.push(p);
                    }
                }
                // members collapsing to one schema need no `z.union` wrapper.
                if parts.len() == 1 {
                    parts.into_iter().next().unwrap()
                } else {
                    format!("z.union([{}])", parts.join(", "))
                }
            }
        }
    }
}

/// If every union member is a [`ir::Ty::Literal`], return their values.
fn literal_union_values(members: &[ir::Ty]) -> Option<Vec<String>> {
    members
        .iter()
        .map(|m| match m {
            ir::Ty::Literal(v) => Some(v.clone()),
            _ => None,
        })
        .collect()
}

/// Map an [`ir::Prim`] to its Zod schema expression.
fn prim_to_zod(p: ir::Prim) -> &'static str {
    match p {
        ir::Prim::String => "z.string()",
        ir::Prim::Int => "z.number().int()",
        ir::Prim::Float => "z.number()",
        ir::Prim::Bool => "z.boolean()",
        ir::Prim::Datetime => "z.string().datetime()",
        ir::Prim::Json => "z.unknown()",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn field(name: &str, ty: ir::Ty, required: bool) -> ir::Field {
        ir::Field {
            name: name.to_string(),
            ty,
            required,
            flexible: false,
            default: None,
            description: None,
            constraints: ir::Constraints::default(),
        }
    }

    #[test]
    fn emits_header() {
        let out = ZodEmitter::new().emit(&ir::Schema::default());
        assert!(out.contains("import { z } from \"zod\";"));
        assert!(out.contains("// DO NOT EDIT MANUALLY"));
    }

    #[test]
    fn emits_enum_as_z_enum() {
        let schema = ir::Schema {
            types: vec![ir::TypeDef::Enum {
                name: "Role".to_string(),
                description: None,
                variants: vec!["admin".to_string(), "member".to_string()],
            }],
            protocol: None,
            ..Default::default()
        };
        let out = ZodEmitter::new().emit(&schema);
        assert!(out.contains("export const Role = z.enum([\"admin\", \"member\"]);"));
    }

    #[test]
    fn emits_object_with_optional_field() {
        let schema = ir::Schema {
            types: vec![ir::TypeDef::Struct {
                name: "User".to_string(),
                description: None,
                fields: vec![
                    field("name", ir::Ty::Primitive(ir::Prim::String), true),
                    field("nick", ir::Ty::Primitive(ir::Prim::String), false),
                ],
            }],
            protocol: None,
            ..Default::default()
        };
        let out = ZodEmitter::new().emit(&schema);
        assert!(out.contains("export const User = z.object({"));
        assert!(out.contains("  name: z.string(),"));
        assert!(out.contains("  nick: z.string().optional(),"));
    }

    #[test]
    fn maps_primitive_and_compound_types() {
        let schema = ir::Schema {
            types: vec![ir::TypeDef::Struct {
                name: "T".to_string(),
                description: None,
                fields: vec![
                    field("n", ir::Ty::Primitive(ir::Prim::Int), true),
                    field("f", ir::Ty::Primitive(ir::Prim::Float), true),
                    field("b", ir::Ty::Primitive(ir::Prim::Bool), true),
                    field("at", ir::Ty::Primitive(ir::Prim::Datetime), true),
                    field("blob", ir::Ty::Primitive(ir::Prim::Json), true),
                    field(
                        "tags",
                        ir::Ty::Array(Box::new(ir::Ty::Primitive(ir::Prim::String))),
                        true,
                    ),
                ],
            }],
            protocol: None,
            ..Default::default()
        };
        let out = ZodEmitter::new().emit(&schema);
        assert!(out.contains("  n: z.number().int(),"));
        assert!(out.contains("  f: z.number(),"));
        assert!(out.contains("  b: z.boolean(),"));
        assert!(out.contains("  at: z.string().datetime(),"));
        assert!(out.contains("  blob: z.unknown(),"));
        assert!(out.contains("  tags: z.array(z.string()),"));
    }

    #[test]
    fn enum_is_emitted_before_referencing_struct() {
        // `User` references `Role`; the schema lists the struct *first*.
        // The emitter must still place the enum before the object so the
        // generated Zod value resolves.
        let schema = ir::Schema {
            types: vec![
                ir::TypeDef::Struct {
                    name: "User".to_string(),
                    description: None,
                    fields: vec![field("role", ir::Ty::Named("Role".to_string()), true)],
                },
                ir::TypeDef::Enum {
                    name: "Role".to_string(),
                    description: None,
                    variants: vec!["admin".to_string()],
                },
            ],
            protocol: None,
            ..Default::default()
        };
        let out = ZodEmitter::new().emit(&schema);
        let enum_pos = out.find("export const Role").expect("enum emitted");
        let struct_pos = out.find("export const User").expect("struct emitted");
        assert!(
            enum_pos < struct_pos,
            "enum must precede the struct using it"
        );
        assert!(out.contains("  role: Role,"));
    }

    #[test]
    fn emits_protocol_payload_schemas() {
        let schema = ir::Schema {
            types: vec![],
            records: vec![],
            relations: vec![],
            protocol: Some(ir::Protocol {
                name: "chat".to_string(),
                version: "1.0.0".to_string(),
                namespace: None,
                description: None,
                channels: vec![ir::Channel {
                    name: "messaging".to_string(),
                    from: ir::ChannelFrom::Client,
                    lifetime: ir::ChannelLifetime::Persistent,
                    backend: ir::ChannelBackend::Stream,
                    channel_id: None,
                    envelope: None,
                    requests: vec![ir::Request {
                        name: "Send".to_string(),
                        fields: vec![field("body", ir::Ty::Primitive(ir::Prim::String), true)],
                        returns: Some(ir::Message {
                            name: "Ack".to_string(),
                            fields: vec![field("id", ir::Ty::Primitive(ir::Prim::String), true)],
                        }),
                    }],
                    events: vec![],
                }],
            }),
        };
        let out = ZodEmitter::new().emit(&schema);
        assert!(out.contains("export const Send = z.object({"));
        assert!(out.contains("export const Ack = z.object({"));
    }

    // -------------------------------------------------------------------------
    // protocol dialect — envelope discriminated union
    // -------------------------------------------------------------------------

    /// The sidebar-IPC spike channel: a `:`-bearing request name, a fieldless
    /// request, and an optional `envelope` tag.
    fn sidebar_schema(envelope: Option<&str>) -> ir::Schema {
        ir::Schema {
            protocol: Some(ir::Protocol {
                name: "sidebar".to_string(),
                version: "1.0.0".to_string(),
                namespace: None,
                description: None,
                channels: vec![ir::Channel {
                    name: "ipc".to_string(),
                    from: ir::ChannelFrom::Client,
                    lifetime: ir::ChannelLifetime::Transient,
                    backend: ir::ChannelBackend::Stream,
                    channel_id: None,
                    envelope: envelope.map(str::to_string),
                    requests: vec![
                        ir::Request {
                            name: "process:toggle".to_string(),
                            fields: vec![field("path", ir::Ty::Primitive(ir::Prim::String), true)],
                            returns: None,
                        },
                        ir::Request {
                            name: "process:add".to_string(),
                            fields: vec![],
                            returns: None,
                        },
                    ],
                    events: vec![],
                }],
            }),
            ..Default::default()
        }
    }

    #[test]
    fn channel_request_schema_names_are_sanitized() {
        // `render_object` PascalCases the schema name, so a `:`-bearing
        // request name yields a valid `export const` identifier.
        let out = ZodEmitter::new().emit(&sidebar_schema(None));
        assert!(out.contains("export const ProcessToggle = z.object({"));
        assert!(out.contains("export const ProcessAdd = z.object({})"));
    }

    #[test]
    fn channel_without_envelope_emits_no_discriminated_union() {
        let out = ZodEmitter::new().emit(&sidebar_schema(None));
        assert!(
            !out.contains("z.discriminatedUnion"),
            "no envelope ⇒ no union"
        );
    }

    #[test]
    fn envelope_channel_emits_discriminated_union() {
        let out = ZodEmitter::new().emit(&sidebar_schema(Some("t")));
        assert!(out.contains("export const IpcEnvelope = z.discriminatedUnion(\"t\", ["));
        assert!(out.contains("  ProcessToggle.extend({ t: z.literal(\"process:toggle\") }),"));
        assert!(out.contains("  ProcessAdd.extend({ t: z.literal(\"process:add\") }),"));
        // the union must follow the per-request objects it references.
        let obj = out.find("export const ProcessToggle").unwrap();
        let union = out.find("export const IpcEnvelope").unwrap();
        assert!(
            obj < union,
            "request objects precede the discriminated union"
        );
    }

    /// A `from="server"` channel carrying only events — the push-only shape.
    fn push_schema(envelope: Option<&str>) -> ir::Schema {
        ir::Schema {
            protocol: Some(ir::Protocol {
                name: "push".to_string(),
                version: "1.0.0".to_string(),
                namespace: None,
                description: None,
                channels: vec![ir::Channel {
                    name: "push".to_string(),
                    from: ir::ChannelFrom::Server,
                    lifetime: ir::ChannelLifetime::Persistent,
                    backend: ir::ChannelBackend::Stream,
                    channel_id: None,
                    envelope: envelope.map(str::to_string),
                    requests: vec![],
                    events: vec![ir::Event {
                        name: "term:ensure_lane".to_string(),
                        fields: vec![field("lane", ir::Ty::Primitive(ir::Prim::String), true)],
                    }],
                }],
            }),
            ..Default::default()
        }
    }

    #[test]
    fn events_only_channel_emits_event_discriminated_union() {
        let out = ZodEmitter::new().emit(&push_schema(Some("t")));
        assert!(out.contains("export const PushEventEnvelope = z.discriminatedUnion(\"t\", ["));
        assert!(out.contains("  TermEnsureLane.extend({ t: z.literal(\"term:ensure_lane\") }),"));
        assert!(
            !out.contains("export const PushEnvelope = "),
            "no requests ⇒ no request union"
        );
        // the union must follow the per-event objects it references.
        let obj = out.find("export const TermEnsureLane").unwrap();
        let union = out.find("export const PushEventEnvelope").unwrap();
        assert!(obj < union, "event objects precede the discriminated union");
    }

    #[test]
    fn channel_with_both_directions_emits_two_discriminated_unions() {
        let mut schema = sidebar_schema(Some("t"));
        schema.protocol.as_mut().unwrap().channels[0].events = vec![ir::Event {
            name: "topic:event".to_string(),
            fields: vec![field("body", ir::Ty::Primitive(ir::Prim::String), true)],
        }];
        let out = ZodEmitter::new().emit(&schema);
        assert!(out.contains("export const IpcEnvelope = z.discriminatedUnion(\"t\", ["));
        assert!(out.contains("export const IpcEventEnvelope = z.discriminatedUnion(\"t\", ["));
        // the event must not leak into the request union.
        let req_union = out.split("export const IpcEnvelope = ").nth(1).unwrap();
        let req_body = req_union.split("]);").next().unwrap();
        assert!(
            !req_body.contains("TopicEvent"),
            "event leaked into the request union"
        );
    }

    #[test]
    fn events_without_envelope_tag_emit_no_union() {
        // Backward compatibility: envelope generation stays opt-in for events too.
        let out = ZodEmitter::new().emit(&push_schema(None));
        assert!(
            !out.contains("z.discriminatedUnion"),
            "no envelope tag ⇒ no union"
        );
        assert!(
            out.contains("export const TermEnsureLane = z.object({"),
            "objects still emit"
        );
    }

    // -------------------------------------------------------------------------
    // Tier 1 — record / relation / link / literal / union
    // -------------------------------------------------------------------------

    #[test]
    fn record_becomes_object_with_id() {
        let schema = ir::Schema {
            records: vec![ir::Record {
                name: "Atlas".to_string(),
                description: None,
                id_strategy: ir::IdStrategy::Uuidv7,
                fields: vec![field("name", ir::Ty::Primitive(ir::Prim::String), true)],
            }],
            ..Default::default()
        };
        let out = ZodEmitter::new().emit(&schema);
        assert!(out.contains("export const Atlas = z.object({"));
        assert!(out.contains("  id: z.string(),"));
        assert!(out.contains("  name: z.string(),"));
    }

    #[test]
    fn relation_object_is_pascal_cased_with_in_out() {
        let schema = ir::Schema {
            relations: vec![ir::Relation {
                name: "derivedFrom".to_string(),
                description: None,
                from: "Memory".to_string(),
                to: "Memory".to_string(),
                unique: false,
                fields: vec![field("reason", ir::Ty::Primitive(ir::Prim::String), false)],
            }],
            ..Default::default()
        };
        let out = ZodEmitter::new().emit(&schema);
        assert!(out.contains("export const DerivedFrom = z.object({"));
        assert!(out.contains("  id: z.string(),"));
        assert!(out.contains("  in: z.string(),"));
        assert!(out.contains("  out: z.string(),"));
        assert!(out.contains("  reason: z.string().optional(),"));
    }

    #[test]
    fn link_becomes_z_string() {
        let schema = ir::Schema {
            records: vec![ir::Record {
                name: "Atlas".to_string(),
                description: None,
                id_strategy: ir::IdStrategy::Uuidv7,
                fields: vec![field("parent", ir::Ty::Link("Atlas".to_string()), true)],
            }],
            ..Default::default()
        };
        let out = ZodEmitter::new().emit(&schema);
        assert!(out.contains("  parent: z.string(),"));
    }

    #[test]
    fn literal_union_collapses_to_z_enum() {
        let schema = ir::Schema {
            types: vec![ir::TypeDef::Struct {
                name: "T".to_string(),
                description: None,
                fields: vec![field(
                    "visibility",
                    ir::Ty::Union(vec![
                        ir::Ty::Literal("public".to_string()),
                        ir::Ty::Literal("private".to_string()),
                    ]),
                    true,
                )],
            }],
            ..Default::default()
        };
        let out = ZodEmitter::new().emit(&schema);
        assert!(out.contains("  visibility: z.enum([\"public\", \"private\"]),"));
    }

    #[test]
    fn mixed_union_becomes_z_union() {
        let schema = ir::Schema {
            types: vec![ir::TypeDef::Struct {
                name: "T".to_string(),
                description: None,
                fields: vec![field(
                    "v",
                    ir::Ty::Union(vec![
                        ir::Ty::Primitive(ir::Prim::String),
                        ir::Ty::Primitive(ir::Prim::Int),
                    ]),
                    true,
                )],
            }],
            ..Default::default()
        };
        let out = ZodEmitter::new().emit(&schema);
        assert!(out.contains("  v: z.union([z.string(), z.number().int()]),"));
    }

    // -------------------------------------------------------------------------
    // Tier 2 — description -> .describe(), constraints -> .min/.max/.regex
    // -------------------------------------------------------------------------

    #[test]
    fn object_and_field_descriptions_become_describe_calls() {
        let mut content = field("content", ir::Ty::Primitive(ir::Prim::String), true);
        content.description = Some("Memory content text".to_string());
        let schema = ir::Schema {
            types: vec![ir::TypeDef::Struct {
                name: "Memory".to_string(),
                description: Some("User memory".to_string()),
                fields: vec![content],
            }],
            ..Default::default()
        };
        let out = ZodEmitter::new().emit(&schema);
        assert!(
            out.contains("}).describe(\"User memory\");"),
            "object .describe()"
        );
        assert!(
            out.contains("z.string().describe(\"Memory content text\")"),
            "field .describe()"
        );
    }

    #[test]
    fn enum_description_becomes_describe_call() {
        let schema = ir::Schema {
            types: vec![ir::TypeDef::Enum {
                name: "Role".to_string(),
                description: Some("An access role".to_string()),
                variants: vec!["admin".to_string()],
            }],
            ..Default::default()
        };
        let out = ZodEmitter::new().emit(&schema);
        assert!(out.contains("z.enum([\"admin\"]).describe(\"An access role\");"));
    }

    #[test]
    fn numeric_constraints_become_min_max() {
        let mut f = field("confidence", ir::Ty::Primitive(ir::Prim::Float), true);
        f.constraints = ir::Constraints {
            min: Some(0),
            max: Some(1),
            ..Default::default()
        };
        let schema = ir::Schema {
            types: vec![ir::TypeDef::Struct {
                name: "T".to_string(),
                description: None,
                fields: vec![f],
            }],
            ..Default::default()
        };
        let out = ZodEmitter::new().emit(&schema);
        assert!(out.contains("  confidence: z.number().min(0).max(1),"));
    }

    #[test]
    fn string_length_and_pattern_constraints_are_emitted() {
        let mut f = field("name", ir::Ty::Primitive(ir::Prim::String), true);
        f.constraints = ir::Constraints {
            min_length: Some(1),
            max_length: Some(32),
            pattern: Some("^[a-z]+$".to_string()),
            ..Default::default()
        };
        let schema = ir::Schema {
            types: vec![ir::TypeDef::Struct {
                name: "T".to_string(),
                description: None,
                fields: vec![f],
            }],
            ..Default::default()
        };
        let out = ZodEmitter::new().emit(&schema);
        assert!(
            out.contains("  name: z.string().min(1).max(32).regex(/^[a-z]+$/),"),
            "got: {out}"
        );
    }

    #[test]
    fn constraint_and_describe_precede_optional_wrapper() {
        let mut f = field("nick", ir::Ty::Primitive(ir::Prim::String), false);
        f.constraints = ir::Constraints {
            max_length: Some(8),
            ..Default::default()
        };
        f.description = Some("nickname".to_string());
        let schema = ir::Schema {
            types: vec![ir::TypeDef::Struct {
                name: "T".to_string(),
                description: None,
                fields: vec![f],
            }],
            ..Default::default()
        };
        let out = ZodEmitter::new().emit(&schema);
        assert!(
            out.contains("z.string().max(8).describe(\"nickname\").optional()"),
            ".optional() must come last; got: {out}"
        );
    }
}
