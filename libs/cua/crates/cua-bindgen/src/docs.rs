//! `cua-bindgen docs --library <lib> [--namespace <ns>]`: the exported API
//! of one UniFFI namespace as JSON.
//!
//! Loads the namespace's ComponentInterface (`cua_sdk` by default; the Cua
//! Spaces app export passes `cua_spaces_ffi`) from the compiled cdylib in
//! library mode (the same pinned `uniffi_bindgen` that generates the
//! bindings), so the reference can never describe an API the library does
//! not export. `scripts/docs-generators/cua-sdk.ts` renders the JSON into
//! the language-tabbed reference under `docs/content/docs/reference/cua-sdk/api`.
//!
//! The output is deterministic: items are sorted by name, fields, arguments
//! and variants keep declaration order, and nothing host-specific (paths,
//! dates, checksums) is emitted.

use std::collections::BTreeMap;

use anyhow::{Context, Result, bail};
use camino::Utf8Path;
use serde_json::{Map, Value, json};
use uniffi_bindgen::interface::{
    Argument, Callable, ComponentInterface, DefaultValue, Enum, Field, Method, Object, Record, Type,
};
use uniffi_bindgen::{EmptyCrateConfigSupplier, library_mode, macro_metadata};
use uniffi_meta::{LiteralMetadata, Metadata};

/// The default UniFFI namespace: the cua SDK. A cdylib also links other
/// namespaces (cyclops-sdk; `libcua_spaces_ffi` links `cua_sdk` as well)
/// that a dump of one namespace leaves out.
pub const NAMESPACE: &str = "cua_sdk";

/// Loads the library and returns the API document of `namespace`.
pub fn dump(library: &Utf8Path, namespace: &str) -> Result<Value> {
    let cis = library_mode::find_cis(library, &EmptyCrateConfigSupplier)
        .with_context(|| format!("reading UniFFI metadata from {library}"))?;
    let Some(ci) = cis.iter().find(|ci| ci.namespace() == namespace) else {
        bail!("{library} does not export the `{namespace}` UniFFI namespace");
    };
    let modules = module_map(
        macro_metadata::extract_from_library(library).context("reading item module paths")?,
        namespace,
    );
    Ok(document(ci, &modules))
}

/// `(kind, name)` -> the Rust module the item is declared in, relative to
/// the crate (`native::sandbox` -> `sandbox`). Items of other namespaces
/// are left out.
fn module_map(items: Vec<Metadata>, namespace: &str) -> BTreeMap<(&'static str, String), String> {
    let mut out = BTreeMap::new();
    for item in items {
        let (kind, name, path) = match &item {
            Metadata::Object(m) => ("object", &m.name, &m.module_path),
            Metadata::Record(m) => ("record", &m.name, &m.module_path),
            Metadata::Enum(m) => ("enum", &m.name, &m.module_path),
            Metadata::CallbackInterface(m) => ("callback", &m.name, &m.module_path),
            Metadata::Func(m) => ("function", &m.name, &m.module_path),
            _ => continue,
        };
        if path.split("::").next() != Some(namespace) {
            continue;
        }
        out.insert((kind, name.clone()), short_module(path));
    }
    out
}

fn short_module(path: &str) -> String {
    let rest: Vec<&str> = path
        .split("::")
        .skip(1)
        .filter(|segment| *segment != "native")
        .collect();
    if rest.is_empty() {
        "root".to_string()
    } else {
        rest.join("::")
    }
}

/// Builds the document from a ComponentInterface (separate from [`dump`]
/// so it is unit-testable without a compiled library).
pub fn document(
    ci: &ComponentInterface,
    modules: &BTreeMap<(&'static str, String), String>,
) -> Value {
    let module = |kind: &'static str, name: &str| {
        modules
            .get(&(kind, name.to_string()))
            .cloned()
            .unwrap_or_else(|| "root".to_string())
    };

    let mut objects: Vec<&Object> = ci.object_definitions().iter().collect();
    objects.sort_by(|a, b| a.name().cmp(b.name()));
    let objects: Vec<Value> = objects
        .into_iter()
        .map(|o| {
            let mut ctors: Vec<Value> = o
                .constructors()
                .into_iter()
                .map(|c| {
                    let mut v = callable(c, c.name());
                    v["primary"] = json!(c.is_primary_constructor());
                    v
                })
                .collect();
            ctors.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
            json!({
                "name": o.name(),
                "module": module("object", o.name()),
                "docstring": o.docstring(),
                "trait_interface": o.is_trait_interface(),
                "constructors": ctors,
                "methods": methods(o.methods()),
            })
        })
        .collect();

    let mut records: Vec<&Record> = ci.record_definitions().iter().collect();
    records.sort_by(|a, b| a.name().cmp(b.name()));
    let records: Vec<Value> = records
        .into_iter()
        .map(|r| {
            json!({
                "name": r.name(),
                "module": module("record", r.name()),
                "docstring": r.docstring(),
                "fields": r.fields().iter().map(field).collect::<Vec<_>>(),
                "methods": methods(r.methods().iter().collect()),
            })
        })
        .collect();

    let mut enums: Vec<&Enum> = ci.enum_definitions().iter().collect();
    enums.sort_by(|a, b| a.name().cmp(b.name()));
    let enums: Vec<Value> = enums
        .into_iter()
        .map(|e| {
            json!({
                "name": e.name(),
                "module": module("enum", e.name()),
                "docstring": e.docstring(),
                "error": ci.is_name_used_as_error(e.name()),
                "flat": e.is_flat(),
                "non_exhaustive": e.is_non_exhaustive(),
                "variants": e.variants().iter().map(|v| json!({
                    "name": v.name(),
                    "docstring": v.docstring(),
                    "fields": v.fields().iter().map(field).collect::<Vec<_>>(),
                })).collect::<Vec<_>>(),
                "methods": methods(e.methods().iter().collect()),
            })
        })
        .collect();

    let mut callbacks: Vec<_> = ci.callback_interface_definitions().iter().collect();
    callbacks.sort_by(|a, b| a.name().cmp(b.name()));
    let callbacks: Vec<Value> = callbacks
        .into_iter()
        .map(|c| {
            json!({
                "name": c.name(),
                "module": module("callback", c.name()),
                "docstring": c.docstring(),
                "methods": methods(c.methods()),
            })
        })
        .collect();

    let mut functions: Vec<_> = ci.function_definitions().iter().collect();
    functions.sort_by(|a, b| a.name().cmp(b.name()));
    let functions: Vec<Value> = functions
        .into_iter()
        .map(|f| {
            let mut v = callable(f, f.name());
            v["module"] = json!(module("function", f.name()));
            v
        })
        .collect();

    json!({
        "namespace": ci.namespace(),
        "docstring": ci.namespace_docstring(),
        "objects": objects,
        "records": records,
        "enums": enums,
        "callback_interfaces": callbacks,
        "functions": functions,
    })
}

fn methods(mut list: Vec<&Method>) -> Vec<Value> {
    list.sort_by(|a, b| a.name().cmp(b.name()));
    list.into_iter().map(|m| callable(m, m.name())).collect()
}

fn callable(c: &dyn Callable, name: &str) -> Value {
    json!({
        "name": name,
        "docstring": c.docstring(),
        "async": c.is_async(),
        "arguments": c.arguments().into_iter().map(argument).collect::<Vec<_>>(),
        "return_type": c.return_type().map(ty),
        "throws": c.throws_type().map(ty),
    })
}

fn argument(a: &Argument) -> Value {
    json!({
        "name": a.name(),
        "type": ty(&uniffi_bindgen::interface::AsType::as_type(a)),
        "default": a.default_value().map(default_value),
    })
}

fn field(f: &Field) -> Value {
    json!({
        "name": f.name(),
        "type": ty(&uniffi_bindgen::interface::AsType::as_type(f)),
        "default": f.default_value().map(default_value),
        "docstring": f.docstring(),
    })
}

/// A language-neutral type: `{"kind": ...}` plus `name` or inner types.
pub fn ty(t: &Type) -> Value {
    let prim = |k: &str| json!({ "kind": k });
    match t {
        Type::UInt8 => prim("u8"),
        Type::Int8 => prim("i8"),
        Type::UInt16 => prim("u16"),
        Type::Int16 => prim("i16"),
        Type::UInt32 => prim("u32"),
        Type::Int32 => prim("i32"),
        Type::UInt64 => prim("u64"),
        Type::Int64 => prim("i64"),
        Type::Float32 => prim("f32"),
        Type::Float64 => prim("f64"),
        Type::Boolean => prim("bool"),
        Type::String => prim("string"),
        Type::Bytes => prim("bytes"),
        Type::Timestamp => prim("timestamp"),
        Type::Duration => prim("duration"),
        Type::Object { name, .. } => json!({ "kind": "object", "name": name }),
        Type::Record { name, .. } => json!({ "kind": "record", "name": name }),
        Type::Enum { name, .. } => json!({ "kind": "enum", "name": name }),
        Type::CallbackInterface { name, .. } => json!({ "kind": "callback", "name": name }),
        Type::Optional { inner_type } => json!({ "kind": "optional", "inner": ty(inner_type) }),
        Type::Sequence { inner_type } => json!({ "kind": "sequence", "inner": ty(inner_type) }),
        Type::Map {
            key_type,
            value_type,
        } => json!({ "kind": "map", "key": ty(key_type), "value": ty(value_type) }),
        Type::Custom { name, builtin, .. } => {
            json!({ "kind": "custom", "name": name, "builtin": ty(builtin) })
        }
    }
}

fn default_value(d: &DefaultValue) -> Value {
    match d {
        DefaultValue::Default => json!({ "kind": "default" }),
        DefaultValue::Literal(l) => literal(l),
    }
}

fn literal(l: &LiteralMetadata) -> Value {
    match l {
        LiteralMetadata::Boolean(b) => json!({ "kind": "bool", "value": b }),
        LiteralMetadata::String(s) => json!({ "kind": "string", "value": s }),
        LiteralMetadata::UInt(v, _, _) => json!({ "kind": "int", "value": v.to_string() }),
        LiteralMetadata::Int(v, _, _) => json!({ "kind": "int", "value": v.to_string() }),
        LiteralMetadata::Float(v, _) => json!({ "kind": "float", "value": v }),
        LiteralMetadata::Enum(v, t) => json!({ "kind": "enum", "value": v, "type": ty(t) }),
        LiteralMetadata::EmptySequence => json!({ "kind": "empty_sequence" }),
        LiteralMetadata::EmptyMap => json!({ "kind": "empty_map" }),
        LiteralMetadata::None => json!({ "kind": "none" }),
        LiteralMetadata::Some { inner } => json!({ "kind": "some", "inner": default_value(inner) }),
    }
}

/// Pretty JSON with a trailing newline.
pub fn render(doc: &Value) -> String {
    let mut out = serde_json::to_string_pretty(&sort_keys(doc)).expect("JSON serialises");
    out.push('\n');
    out
}

/// Sorts object keys regardless of serde_json's `preserve_order` feature
/// (which another workspace crate may enable through feature unification).
fn sort_keys(v: &Value) -> Value {
    match v {
        Value::Object(map) => {
            let sorted: BTreeMap<&String, Value> =
                map.iter().map(|(k, v)| (k, sort_keys(v))).collect();
            let mut out = Map::new();
            for (k, v) in sorted {
                out.insert(k.clone(), v);
            }
            Value::Object(out)
        }
        Value::Array(items) => Value::Array(items.iter().map(sort_keys).collect()),
        other => other.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const UDL: &str = r#"
        namespace demo {
            /// Adds.
            [Throws=DemoError]
            u32 add(u32 a, optional u32 b = 2);
        };
        /// A sandbox-like handle.
        interface Handle {
            constructor(string name);
            [Async] sequence<string> list(record<string, i64> filters);
        };
        dictionary Info {
            string name;
            boolean ready = false;
        };
        [Error]
        enum DemoError { "NotFound", "Denied" };
        enum Kind { "Container", "Vm" };
    "#;

    #[test]
    fn document_is_deterministic_and_complete() {
        let ci = ComponentInterface::from_webidl(UDL, "demo").expect("UDL parses");
        let mut modules = BTreeMap::new();
        modules.insert(("object", "Handle".to_string()), "sandbox".to_string());
        let doc = document(&ci, &modules);
        let text = render(&doc);
        assert_eq!(text, render(&document(&ci, &modules)));

        assert_eq!(doc["namespace"], "demo");
        let add = &doc["functions"][0];
        assert_eq!(add["name"], "add");
        assert_eq!(add["docstring"], "Adds.");
        assert_eq!(
            add["throws"],
            json!({ "kind": "enum", "name": "DemoError" })
        );
        assert_eq!(add["arguments"][1]["type"]["kind"], "u32");
        assert_eq!(add["arguments"][1]["default"]["value"], "2");

        let handle = &doc["objects"][0];
        assert_eq!(handle["module"], "sandbox");
        assert_eq!(handle["docstring"], "A sandbox-like handle.");
        assert_eq!(handle["constructors"][0]["primary"], true);
        let list = &handle["methods"][0];
        assert_eq!(list["async"], true);
        assert_eq!(list["arguments"][0]["type"]["kind"], "map");
        assert_eq!(list["return_type"]["inner"]["kind"], "string");

        let errors: Vec<_> = doc["enums"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| (e["name"].as_str().unwrap(), e["error"].as_bool().unwrap()))
            .collect();
        assert_eq!(errors, vec![("DemoError", true), ("Kind", false)]);
        assert_eq!(doc["records"][0]["fields"][1]["default"]["value"], false);
        assert_eq!(doc["records"][0]["module"], "root");
        // Keys are sorted, so the JSON is stable across serde_json features.
        assert!(
            text.find("\"callback_interfaces\"").unwrap() < text.find("\"docstring\"").unwrap()
        );
    }

    #[test]
    fn module_paths_are_crate_relative() {
        assert_eq!(short_module("cua_sdk::native::sandbox"), "sandbox");
        assert_eq!(short_module("cua_sdk"), "root");
        assert_eq!(
            short_module("cua_sdk::native::teleport_types"),
            "teleport_types"
        );
        // The Cua Spaces app export (`--namespace cua_spaces_ffi`).
        assert_eq!(short_module("cua_spaces_ffi::teleport_app"), "teleport_app");
        assert_eq!(short_module("cua_spaces_ffi"), "root");
    }
}
