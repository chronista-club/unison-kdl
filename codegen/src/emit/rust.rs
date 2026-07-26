//! Rust emitter — renders [`ir::Schema`] into Rust source text.
//!
//! Ported from club-unison's `codegen/rust.rs`. The original used
//! `proc_macro2` + `quote` to build a `TokenStream` and a hand-rolled
//! `format_code` pass; this port writes pre-formatted Rust text directly so
//! `club-kdl-codegen` stays dependency-free during Phase 1.
//!
//! ## What it emits
//!
//! - data dialect: every [`ir::TypeDef`] — `struct` (with fields) and `enum`
//!   (string-valued variants).
//! - entity dialect: every [`ir::Record`] as a `struct` carrying an `id`
//!   field; every [`ir::Relation`] as an edge `struct` carrying `id` / `in` /
//!   `out` fields plus its edge properties.
//! - protocol dialect: for every [`ir::Channel`], a `struct` per request
//!   payload, per `returns` message, and per event payload.
//!
//! ## Tier 1 type mapping
//!
//! - `link<Record>` → `String` (the linked record's id).
//! - `'literal'` and unions of literals → a generated string-valued `enum`
//!   is *not* produced (no schema name is available at the field site);
//!   instead the field type degrades to `String`. A union of non-literal
//!   types also degrades to `serde_json::Value` — Rust has no anonymous sum
//!   type, and inventing names per field is out of Tier 1 scope.
//!
//! Each generated `struct` / `enum` carries `#[derive(...)]` attributes and
//! `serde` annotations matching club-unison's generator. Optional fields
//! become `Option<T>` with `#[serde(skip_serializing_if = "Option::is_none")]`.
//!
//! ## Differences from club-unison (IR-driven port)
//!
//! - The IR has no inline `_inline_*` messages and no `service` / `method` /
//!   `stream` / `send` / `recv` legacy constructs — so the corresponding
//!   branches are dropped.
//! - The IR's [`ir::Prim::Datetime`] maps to `chrono::DateTime<Utc>`; named
//!   type references emit the bare identifier (no `TypeRegistry` indirection).
//!
//! ## Tier 2 — description / constraints
//!
//! - A `description` on a `struct` / `enum` / `record` / `relation` or a
//!   field becomes a `///` doc comment.
//! - Field `constraints` (`min` / `max` / `min_length` / `max_length` /
//!   `pattern`) are **not** emitted — Rust's type system cannot express them,
//!   and JSDoc-style `@minimum` hacks are deliberately avoided.

use crate::Emitter;
use crate::ir;

use super::case::{to_pascal_case, to_snake_case};

/// The Rust code generation target.
#[derive(Debug, Default, Clone, Copy)]
pub struct RustEmitter;

impl RustEmitter {
    /// Create a new [`RustEmitter`].
    pub fn new() -> Self {
        Self
    }
}

impl Emitter for RustEmitter {
    fn emit(&self, schema: &ir::Schema) -> String {
        let mut out = String::new();
        out.push_str(IMPORTS);

        // data dialect — standalone type definitions.
        for ty in &schema.types {
            out.push('\n');
            out.push_str(&render_typedef(ty));
        }

        // entity dialect — records and relations.
        for record in &schema.records {
            out.push('\n');
            out.push_str(&render_record(record));
        }
        for relation in &schema.relations {
            out.push('\n');
            out.push_str(&render_relation(relation));
        }

        // protocol dialect — channel payload structs.
        if let Some(protocol) = &schema.protocol {
            for channel in &protocol.channels {
                out.push_str(&render_channel(channel));
            }
        }

        out
    }
}

/// Header import block, matching club-unison's `generate_imports`.
const IMPORTS: &str = "\
use serde::{Deserialize, Serialize};
use anyhow::Result;
use chrono::{DateTime, Utc};
use uuid::Uuid;
use std::collections::HashMap;
";

/// Render one standalone [`ir::TypeDef`].
fn render_typedef(ty: &ir::TypeDef) -> String {
    match ty {
        ir::TypeDef::Struct {
            name,
            description,
            fields,
        } => render_struct(name, description.as_deref(), fields),
        ir::TypeDef::Enum {
            name,
            description,
            variants,
        } => render_enum(name, description.as_deref(), variants),
    }
}

/// Render a `///` doc comment block at the given indentation from an optional
/// description. Each line of a multi-line description gets its own `///`.
fn render_doc(description: Option<&str>, indent: &str) -> String {
    match description {
        Some(text) => text
            .lines()
            .map(|line| format!("{indent}/// {line}\n"))
            .collect(),
        None => String::new(),
    }
}

/// Render a `struct` from a name and field list. A fieldless struct becomes a
/// unit struct (`pub struct Name;`), matching club-unison.
fn render_struct(name: &str, description: Option<&str>, fields: &[ir::Field]) -> String {
    let derive = "#[derive(Debug, Clone, Serialize, Deserialize)]\n";
    let doc = render_doc(description, "");
    if fields.is_empty() {
        return format!("{doc}{derive}pub struct {name};\n");
    }
    let mut out = String::new();
    out.push_str(&doc);
    out.push_str(derive);
    out.push_str(&format!("pub struct {name} {{\n"));
    for field in fields {
        out.push_str(&render_field(field));
    }
    out.push_str("}\n");
    out
}

/// Render an `enum` of string-valued variants.
fn render_enum(name: &str, description: Option<&str>, variants: &[String]) -> String {
    let mut out = String::new();
    out.push_str(&render_doc(description, ""));
    out.push_str("#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]\n");
    out.push_str("#[serde(rename_all = \"snake_case\")]\n");
    out.push_str(&format!("pub enum {name} {{\n"));
    for v in variants {
        out.push_str(&format!("    #[serde(rename = \"{v}\")]\n"));
        out.push_str(&format!("    {},\n", to_pascal_case(v)));
    }
    out.push_str("}\n");
    out
}

/// Render a single struct field with its `serde` attributes.
fn render_field(field: &ir::Field) -> String {
    let mut out = String::new();

    // `///` doc comment from the field description.
    out.push_str(&render_doc(field.description.as_deref(), "    "));

    // `#[serde(rename = "...")]` when the source name is not snake_case.
    let snake = to_snake_case(&field.name);
    if field.name != snake {
        out.push_str(&format!("    #[serde(rename = \"{}\")]\n", field.name));
    }

    let base = ty_to_rust(&field.ty);
    let rust_ty = if field.required {
        base
    } else {
        out.push_str("    #[serde(skip_serializing_if = \"Option::is_none\")]\n");
        format!("Option<{base}>")
    };

    out.push_str(&format!(
        "    pub {}: {rust_ty},\n",
        field_ident(&field.name)
    ));
    out
}

/// Render a field name as a valid Rust identifier. A name that collides with
/// a Rust keyword is escaped as a raw identifier (`r#type`) so the generated
/// source compiles. `serde` strips the `r#` prefix, so the wire name is
/// unaffected.
///
/// `crate` / `self` / `Self` / `super` cannot be raw identifiers; they are
/// left as-is (a KDL schema field is extremely unlikely to use them).
fn field_ident(name: &str) -> String {
    const KEYWORDS: &[&str] = &[
        "as", "break", "const", "continue", "dyn", "else", "enum", "extern", "false", "fn", "for",
        "if", "impl", "in", "let", "loop", "match", "mod", "move", "mut", "pub", "ref", "return",
        "static", "struct", "trait", "true", "type", "unsafe", "use", "where", "while", "async",
        "await", "gen", "abstract", "become", "box", "do", "final", "macro", "override", "priv",
        "try", "typeof", "unsized", "virtual", "yield",
    ];
    if KEYWORDS.contains(&name) {
        format!("r#{name}")
    } else {
        name.to_string()
    }
}

/// Map an [`ir::Ty`] to its Rust type expression.
fn ty_to_rust(ty: &ir::Ty) -> String {
    match ty {
        ir::Ty::Primitive(p) => prim_to_rust(*p).to_string(),
        ir::Ty::Array(inner) => format!("Vec<{}>", ty_to_rust(inner)),
        ir::Ty::Named(name) => name.clone(),
        // a link stores the target record's id — a plain string.
        ir::Ty::Link(_) => "String".to_string(),
        // a literal degrades to String (no per-field type name available).
        ir::Ty::Literal(_) => "String".to_string(),
        // a union of string literals stays a String. Otherwise, if every
        // member maps to the same Rust type, collapse to it (`link<X> |
        // string` → `String`); a genuinely heterogeneous union has no Rust
        // anonymous-sum representation, so it degrades to a JSON value.
        ir::Ty::Union(members) => {
            if members.iter().all(|m| matches!(m, ir::Ty::Literal(_))) {
                "String".to_string()
            } else {
                let mut mapped: Vec<String> = Vec::new();
                for m in members {
                    let t = ty_to_rust(m);
                    if !mapped.contains(&t) {
                        mapped.push(t);
                    }
                }
                if mapped.len() == 1 {
                    mapped.into_iter().next().unwrap()
                } else {
                    "serde_json::Value".to_string()
                }
            }
        }
    }
}

/// Map an [`ir::Prim`] to its Rust type.
fn prim_to_rust(p: ir::Prim) -> &'static str {
    match p {
        ir::Prim::String => "String",
        ir::Prim::Int => "i64",
        ir::Prim::Float => "f64",
        ir::Prim::Bool => "bool",
        ir::Prim::Datetime => "DateTime<Utc>",
        ir::Prim::Json => "serde_json::Value",
    }
}

/// Render one [`ir::Record`] as a `struct`. The record's `id` becomes a
/// leading `String` field; the remaining fields follow in source order. The
/// struct name is PascalCased so a camelCase record name still compiles.
fn render_record(record: &ir::Record) -> String {
    let mut fields = Vec::with_capacity(record.fields.len() + 1);
    fields.push(id_field());
    fields.extend(record.fields.iter().cloned());
    render_struct(
        &to_pascal_case(&record.name),
        record.description.as_deref(),
        &fields,
    )
}

/// Render one [`ir::Relation`] as an edge `struct` carrying `id` / `in` /
/// `out` (the edge endpoints, as record ids) plus its edge-property fields.
fn render_relation(relation: &ir::Relation) -> String {
    let mut fields = Vec::with_capacity(relation.fields.len() + 3);
    fields.push(id_field());
    fields.push(ir::Field {
        name: "in".to_string(),
        ty: ir::Ty::Primitive(ir::Prim::String),
        required: true,
        flexible: false,
        default: None,
        description: None,
        constraints: ir::Constraints::default(),
    });
    fields.push(ir::Field {
        name: "out".to_string(),
        ty: ir::Ty::Primitive(ir::Prim::String),
        required: true,
        flexible: false,
        default: None,
        description: None,
        constraints: ir::Constraints::default(),
    });
    fields.extend(relation.fields.iter().cloned());
    render_struct(
        &to_pascal_case(&relation.name),
        relation.description.as_deref(),
        &fields,
    )
}

/// The synthetic `id: String` field shared by records and relations.
fn id_field() -> ir::Field {
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

/// Render every payload struct for one channel: request payloads, `returns`
/// messages, and event payloads. Payload names are PascalCased so a wire-style
/// schema name (`process:toggle`) becomes a valid Rust identifier
/// (`ProcessToggle`).
///
/// When the channel declares `envelope="<tag>"`, discriminated-union enums
/// bundling the channel's payloads are appended — one over its requests
/// (`{Channel}Envelope`) and one over its events (`{Channel}EventEnvelope`).
/// See [`render_envelope_enum`].
fn render_channel(channel: &ir::Channel) -> String {
    let mut out = String::new();
    for req in &channel.requests {
        out.push('\n');
        out.push_str(&render_struct(
            &to_pascal_case(&req.name),
            None,
            &req.fields,
        ));
        if let Some(returns) = &req.returns {
            out.push('\n');
            out.push_str(&render_struct(
                &to_pascal_case(&returns.name),
                None,
                &returns.fields,
            ));
        }
    }
    for evt in &channel.events {
        out.push('\n');
        out.push_str(&render_struct(
            &to_pascal_case(&evt.name),
            None,
            &evt.fields,
        ));
    }
    if let Some(tag) = &channel.envelope {
        if !channel.requests.is_empty() {
            let members: Vec<(&str, &[ir::Field])> = channel
                .requests
                .iter()
                .map(|req| (req.name.as_str(), req.fields.as_slice()))
                .collect();
            out.push('\n');
            out.push_str(&render_envelope_enum(
                &channel.name,
                tag,
                &format!("{}Envelope", to_pascal_case(&channel.name)),
                "requests",
                &members,
            ));
        }
        // Events get their own envelope: a channel can carry both directions
        // (a `from="client"` channel whose server pushes events back), and the
        // two sets are dispatched by different peers. Bundling them into one
        // union would force each side to match arms it can never receive.
        if !channel.events.is_empty() {
            let members: Vec<(&str, &[ir::Field])> = channel
                .events
                .iter()
                .map(|evt| (evt.name.as_str(), evt.fields.as_slice()))
                .collect();
            out.push('\n');
            out.push_str(&render_envelope_enum(
                &channel.name,
                tag,
                &format!("{}EventEnvelope", to_pascal_case(&channel.name)),
                "events",
                &members,
            ));
        }
    }
    out
}

/// Render an envelope `enum`: an internally `#[serde(tag = "...")]`
/// discriminated union bundling one direction's payloads.
///
/// A member carrying fields becomes a newtype variant wrapping its payload
/// struct (`ProcessToggle(ProcessToggle)`); a fieldless member becomes a unit
/// variant (`ProcessAdd`). The unit form is required — serde rejects an
/// internally tagged newtype variant that wraps a unit struct at runtime.
///
/// The variant identifier is the PascalCased member name; the original
/// (possibly `:`-bearing) wire name is preserved with `#[serde(rename = ...)]`
/// whenever sanitizing changed it.
///
/// `members` is `(wire name, payload fields)` in source order — requests and
/// events share this shape, so both directions render through one path.
/// `member_kind` names the direction in the doc comment (`"requests"` /
/// `"events"`).
fn render_envelope_enum(
    channel_name: &str,
    tag: &str,
    enum_name: &str,
    member_kind: &str,
    members: &[(&str, &[ir::Field])],
) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "/// Envelope enum for channel {channel_name:?} — a discriminated union over its\n\
         /// {member_kind}, internally tagged by the {tag:?} field.\n"
    ));
    out.push_str("#[derive(Debug, Clone, Serialize, Deserialize)]\n");
    out.push_str(&format!("#[serde(tag = \"{tag}\")]\n"));
    out.push_str(&format!("pub enum {enum_name} {{\n"));
    for (name, fields) in members {
        let variant = to_pascal_case(name);
        if variant != *name {
            out.push_str(&format!("    #[serde(rename = \"{name}\")]\n"));
        }
        if fields.is_empty() {
            out.push_str(&format!("    {variant},\n"));
        } else {
            out.push_str(&format!("    {variant}({variant}),\n"));
        }
    }
    out.push_str("}\n");
    out
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
    fn emits_import_header() {
        let out = RustEmitter::new().emit(&ir::Schema::default());
        assert!(out.contains("use serde::{Deserialize, Serialize};"));
        assert!(out.contains("use chrono::{DateTime, Utc};"));
    }

    #[test]
    fn emits_struct_with_required_field() {
        let schema = ir::Schema {
            types: vec![ir::TypeDef::Struct {
                name: "User".to_string(),
                description: None,
                fields: vec![field("name", ir::Ty::Primitive(ir::Prim::String), true)],
            }],
            protocol: None,
            ..Default::default()
        };
        let out = RustEmitter::new().emit(&schema);
        assert!(out.contains("#[derive(Debug, Clone, Serialize, Deserialize)]"));
        assert!(out.contains("pub struct User {"));
        assert!(out.contains("    pub name: String,"));
    }

    #[test]
    fn keyword_field_name_is_raw_identifier() {
        let schema = ir::Schema {
            types: vec![ir::TypeDef::Struct {
                name: "Node".to_string(),
                description: None,
                fields: vec![field("type", ir::Ty::Primitive(ir::Prim::String), true)],
            }],
            protocol: None,
            ..Default::default()
        };
        let out = RustEmitter::new().emit(&schema);
        // `type` is a Rust keyword — it must be escaped as a raw identifier
        // so the generated source compiles.
        assert!(out.contains("pub r#type: String,"));
    }

    #[test]
    fn optional_field_becomes_option_with_skip() {
        let schema = ir::Schema {
            types: vec![ir::TypeDef::Struct {
                name: "User".to_string(),
                description: None,
                fields: vec![field("nick", ir::Ty::Primitive(ir::Prim::String), false)],
            }],
            protocol: None,
            ..Default::default()
        };
        let out = RustEmitter::new().emit(&schema);
        assert!(out.contains("#[serde(skip_serializing_if = \"Option::is_none\")]"));
        assert!(out.contains("pub nick: Option<String>,"));
    }

    #[test]
    fn non_snake_field_gets_serde_rename() {
        let schema = ir::Schema {
            types: vec![ir::TypeDef::Struct {
                name: "User".to_string(),
                description: None,
                fields: vec![field(
                    "displayName",
                    ir::Ty::Primitive(ir::Prim::String),
                    true,
                )],
            }],
            protocol: None,
            ..Default::default()
        };
        let out = RustEmitter::new().emit(&schema);
        assert!(out.contains("#[serde(rename = \"displayName\")]"));
        assert!(out.contains("pub displayName: String,"));
    }

    #[test]
    fn fieldless_struct_is_unit() {
        let schema = ir::Schema {
            types: vec![ir::TypeDef::Struct {
                name: "Empty".to_string(),
                description: None,
                fields: vec![],
            }],
            protocol: None,
            ..Default::default()
        };
        let out = RustEmitter::new().emit(&schema);
        assert!(out.contains("pub struct Empty;"));
    }

    #[test]
    fn emits_enum_with_rename() {
        let schema = ir::Schema {
            types: vec![ir::TypeDef::Enum {
                name: "Role".to_string(),
                description: None,
                variants: vec!["admin".to_string(), "guest_user".to_string()],
            }],
            protocol: None,
            ..Default::default()
        };
        let out = RustEmitter::new().emit(&schema);
        assert!(out.contains("#[serde(rename_all = \"snake_case\")]"));
        assert!(out.contains("pub enum Role {"));
        assert!(out.contains("#[serde(rename = \"admin\")]"));
        assert!(out.contains("    Admin,"));
        assert!(out.contains("#[serde(rename = \"guest_user\")]"));
        assert!(out.contains("    GuestUser,"));
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
                    field("owner", ir::Ty::Named("User".to_string()), true),
                ],
            }],
            protocol: None,
            ..Default::default()
        };
        let out = RustEmitter::new().emit(&schema);
        assert!(out.contains("pub n: i64,"));
        assert!(out.contains("pub f: f64,"));
        assert!(out.contains("pub b: bool,"));
        assert!(out.contains("pub at: DateTime<Utc>,"));
        assert!(out.contains("pub blob: serde_json::Value,"));
        assert!(out.contains("pub tags: Vec<String>,"));
        assert!(out.contains("pub owner: User,"));
    }

    #[test]
    fn emits_channel_request_returns_and_event_structs() {
        let schema = ir::Schema {
            types: vec![],
            records: vec![],
            relations: vec![],
            protocol: Some(ir::Protocol {
                name: "ping-pong".to_string(),
                version: "2.0.0".to_string(),
                namespace: None,
                description: None,
                channels: vec![ir::Channel {
                    name: "ping-pong".to_string(),
                    from: ir::ChannelFrom::Client,
                    lifetime: ir::ChannelLifetime::Persistent,
                    backend: ir::ChannelBackend::Stream,
                    channel_id: None,
                    envelope: None,
                    requests: vec![ir::Request {
                        name: "Ping".to_string(),
                        fields: vec![field("seq", ir::Ty::Primitive(ir::Prim::Int), true)],
                        returns: Some(ir::Message {
                            name: "Pong".to_string(),
                            fields: vec![field("seq", ir::Ty::Primitive(ir::Prim::Int), true)],
                        }),
                    }],
                    events: vec![ir::Event {
                        name: "Tick".to_string(),
                        fields: vec![],
                    }],
                }],
            }),
        };
        let out = RustEmitter::new().emit(&schema);
        assert!(out.contains("pub struct Ping {"));
        assert!(out.contains("pub struct Pong {"));
        assert!(out.contains("pub struct Tick;"));
    }

    // -------------------------------------------------------------------------
    // protocol dialect — envelope enum + identifier sanitize
    // -------------------------------------------------------------------------

    /// The sidebar-IPC spike channel: `:`-bearing request names, a fieldless
    /// request, and an `envelope` tag.
    fn sidebar_channel(envelope: Option<&str>) -> ir::Channel {
        ir::Channel {
            name: "ipc".to_string(),
            from: ir::ChannelFrom::Client,
            lifetime: ir::ChannelLifetime::Transient,
            backend: ir::ChannelBackend::Stream,
            channel_id: None,
            envelope: envelope.map(str::to_string),
            requests: vec![
                ir::Request {
                    name: "process:toggle".to_string(),
                    fields: vec![
                        field("path", ir::Ty::Primitive(ir::Prim::String), true),
                        field("expanded", ir::Ty::Primitive(ir::Prim::Bool), true),
                    ],
                    returns: None,
                },
                ir::Request {
                    name: "process:add".to_string(),
                    fields: vec![],
                    returns: None,
                },
            ],
            events: vec![],
        }
    }

    fn protocol_schema(channel: ir::Channel) -> ir::Schema {
        ir::Schema {
            protocol: Some(ir::Protocol {
                name: "sidebar".to_string(),
                version: "1.0.0".to_string(),
                namespace: None,
                description: None,
                channels: vec![channel],
            }),
            ..Default::default()
        }
    }

    #[test]
    fn channel_request_names_are_sanitized_to_valid_identifiers() {
        // A `:`-bearing request name must not leak into `pub struct foo:bar`.
        let out = RustEmitter::new().emit(&protocol_schema(sidebar_channel(None)));
        assert!(out.contains("pub struct ProcessToggle {"));
        assert!(out.contains("pub struct ProcessAdd;"));
        assert!(
            !out.contains("process:toggle"),
            "raw `:` name must not leak"
        );
    }

    #[test]
    fn channel_without_envelope_emits_no_enum() {
        // Backward compatibility: an `envelope`-less channel emits only structs.
        let out = RustEmitter::new().emit(&protocol_schema(sidebar_channel(None)));
        assert!(!out.contains("pub enum"), "no envelope ⇒ no enum");
    }

    #[test]
    fn envelope_channel_emits_internally_tagged_enum() {
        let out = RustEmitter::new().emit(&protocol_schema(sidebar_channel(Some("t"))));
        assert!(out.contains("#[serde(tag = \"t\")]"), "internally tagged");
        assert!(
            out.contains("pub enum IpcEnvelope {"),
            "enum named <Channel>Envelope"
        );
        // a request with fields → newtype variant wrapping its struct.
        assert!(out.contains("    #[serde(rename = \"process:toggle\")]"));
        assert!(out.contains("    ProcessToggle(ProcessToggle),"));
        // a fieldless request → unit variant (serde rejects newtype-of-unit).
        assert!(out.contains("    #[serde(rename = \"process:add\")]"));
        assert!(out.contains("    ProcessAdd,\n"));
        assert!(
            !out.contains("ProcessAdd(ProcessAdd)"),
            "fieldless request must not become a newtype variant"
        );
    }

    /// A `from="server"` channel that carries only events — the push-only shape
    /// (Rust → webview, pubsub fan-out). Before events were enveloped, such a
    /// channel emitted payload structs but nothing to dispatch on.
    fn push_channel(envelope: Option<&str>) -> ir::Channel {
        ir::Channel {
            name: "push".to_string(),
            from: ir::ChannelFrom::Server,
            lifetime: ir::ChannelLifetime::Persistent,
            backend: ir::ChannelBackend::Stream,
            channel_id: None,
            envelope: envelope.map(str::to_string),
            requests: vec![],
            events: vec![
                ir::Event {
                    name: "term:ensure_lane".to_string(),
                    fields: vec![
                        field("lane", ir::Ty::Primitive(ir::Prim::String), true),
                        field("session", ir::Ty::Primitive(ir::Prim::Int), true),
                    ],
                },
                ir::Event {
                    name: "term:clear".to_string(),
                    fields: vec![],
                },
            ],
        }
    }

    #[test]
    fn events_only_channel_emits_event_envelope() {
        let out = RustEmitter::new().emit(&protocol_schema(push_channel(Some("t"))));
        assert!(out.contains("#[serde(tag = \"t\")]"), "internally tagged");
        assert!(
            out.contains("pub enum PushEventEnvelope {"),
            "enum named <Channel>EventEnvelope"
        );
        // an event with fields → newtype variant wrapping its struct.
        assert!(out.contains("    #[serde(rename = \"term:ensure_lane\")]"));
        assert!(out.contains("    TermEnsureLane(TermEnsureLane),"));
        // a fieldless event → unit variant (serde rejects newtype-of-unit).
        assert!(out.contains("    TermClear,\n"));
        assert!(
            !out.contains("TermClear(TermClear)"),
            "fieldless event must not become a newtype variant"
        );
        // request-side envelope is not emitted for a request-less channel.
        assert!(
            !out.contains("pub enum PushEnvelope {"),
            "no requests ⇒ no request envelope"
        );
    }

    #[test]
    fn channel_with_both_directions_emits_two_envelopes() {
        // A `from="client"` channel whose server pushes events back (unison's
        // pubsub shape). The two sets are dispatched by different peers, so each
        // gets its own union rather than one mixed enum.
        let mut channel = sidebar_channel(Some("t"));
        channel.events = vec![ir::Event {
            name: "topic:event".to_string(),
            fields: vec![field("body", ir::Ty::Primitive(ir::Prim::String), true)],
        }];
        let out = RustEmitter::new().emit(&protocol_schema(channel));
        assert!(out.contains("pub enum IpcEnvelope {"), "requests envelope");
        assert!(
            out.contains("pub enum IpcEventEnvelope {"),
            "events envelope"
        );
        // the event must not leak into the request envelope.
        let req_enum = out.split("pub enum IpcEnvelope {").nth(1).unwrap();
        let req_body = req_enum.split("}\n").next().unwrap();
        assert!(
            !req_body.contains("TopicEvent"),
            "event leaked into the request envelope"
        );
    }

    #[test]
    fn events_without_envelope_tag_emit_no_enum() {
        // Backward compatibility: envelope generation stays opt-in for events too.
        let out = RustEmitter::new().emit(&protocol_schema(push_channel(None)));
        assert!(!out.contains("pub enum"), "no envelope tag ⇒ no enum");
        assert!(
            out.contains("pub struct TermEnsureLane"),
            "structs still emit"
        );
    }

    #[test]
    fn envelope_variant_without_colon_name_needs_no_rename() {
        // A request whose name is already PascalCase carries no `#[serde(rename)]`.
        let mut channel = sidebar_channel(Some("t"));
        channel.requests = vec![ir::Request {
            name: "Ping".to_string(),
            fields: vec![field("seq", ir::Ty::Primitive(ir::Prim::Int), true)],
            returns: None,
        }];
        let out = RustEmitter::new().emit(&protocol_schema(channel));
        assert!(out.contains("    Ping(Ping),"));
        // `Ping` == to_pascal_case("Ping") → no rename attribute precedes it.
        let variant_line = out.find("    Ping(Ping),").unwrap();
        let preceding = &out[..variant_line];
        assert!(
            !preceding.trim_end().ends_with("rename = \"Ping\")]"),
            "an already-PascalCase name needs no rename"
        );
    }

    // -------------------------------------------------------------------------
    // Tier 1 — record / relation / link / union
    // -------------------------------------------------------------------------

    #[test]
    fn record_becomes_struct_with_id_field() {
        let schema = ir::Schema {
            records: vec![ir::Record {
                name: "Atlas".to_string(),
                description: None,
                id_strategy: ir::IdStrategy::Uuidv7,
                fields: vec![field("name", ir::Ty::Primitive(ir::Prim::String), true)],
            }],
            ..Default::default()
        };
        let out = RustEmitter::new().emit(&schema);
        assert!(out.contains("pub struct Atlas {"));
        assert!(out.contains("pub id: String,"), "record gets an id field");
        assert!(out.contains("pub name: String,"));
    }

    #[test]
    fn relation_becomes_edge_struct_with_in_out() {
        let schema = ir::Schema {
            relations: vec![ir::Relation {
                name: "derivedFrom".to_string(),
                description: None,
                from: "Memory".to_string(),
                to: "Memory".to_string(),
                unique: true,
                fields: vec![field("reason", ir::Ty::Primitive(ir::Prim::String), false)],
            }],
            ..Default::default()
        };
        let out = RustEmitter::new().emit(&schema);
        assert!(out.contains("pub struct DerivedFrom {"));
        assert!(out.contains("pub id: String,"));
        // `in` is a Rust keyword → escaped as a raw identifier.
        assert!(out.contains("pub r#in: String,"));
        assert!(out.contains("pub out: String,"));
        assert!(out.contains("pub reason: Option<String>,"));
    }

    #[test]
    fn link_field_becomes_string() {
        let schema = ir::Schema {
            records: vec![ir::Record {
                name: "Atlas".to_string(),
                description: None,
                id_strategy: ir::IdStrategy::Uuidv7,
                fields: vec![field("parent", ir::Ty::Link("Atlas".to_string()), false)],
            }],
            ..Default::default()
        };
        let out = RustEmitter::new().emit(&schema);
        assert!(out.contains("pub parent: Option<String>,"));
    }

    #[test]
    fn literal_union_degrades_to_string() {
        let schema = ir::Schema {
            records: vec![ir::Record {
                name: "Doc".to_string(),
                description: None,
                id_strategy: ir::IdStrategy::Uuidv7,
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
        let out = RustEmitter::new().emit(&schema);
        assert!(out.contains("pub visibility: String,"));
    }

    #[test]
    fn mixed_union_degrades_to_json_value() {
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
        let out = RustEmitter::new().emit(&schema);
        assert!(out.contains("pub v: serde_json::Value,"));
    }

    // -------------------------------------------------------------------------
    // Tier 2 — description → `///` doc comments (constraints are not emitted)
    // -------------------------------------------------------------------------

    #[test]
    fn struct_and_field_descriptions_become_doc_comments() {
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
        let out = RustEmitter::new().emit(&schema);
        assert!(out.contains("/// User memory\n"), "struct doc comment");
        assert!(
            out.contains("    /// Memory content text\n"),
            "field doc comment"
        );
    }

    #[test]
    fn enum_description_becomes_doc_comment() {
        let schema = ir::Schema {
            types: vec![ir::TypeDef::Enum {
                name: "Role".to_string(),
                description: Some("An access role".to_string()),
                variants: vec!["admin".to_string()],
            }],
            ..Default::default()
        };
        let out = RustEmitter::new().emit(&schema);
        assert!(out.contains("/// An access role\n"));
    }

    #[test]
    fn constraints_do_not_appear_in_rust_output() {
        // Rust's type system cannot express min/max/pattern — they must be
        // dropped, not emitted as attributes or comments.
        let mut f = field("confidence", ir::Ty::Primitive(ir::Prim::Float), true);
        f.constraints = ir::Constraints {
            min: Some(0),
            max: Some(1),
            pattern: Some("x".to_string()),
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
        let out = RustEmitter::new().emit(&schema);
        assert!(out.contains("pub confidence: f64,"));
        assert!(!out.contains("minimum"), "no constraint metadata leaks");
    }
}
