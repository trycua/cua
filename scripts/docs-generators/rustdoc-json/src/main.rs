//! Dumps the public API of the cua Rust crates as a small, deterministic JSON
//! document for `scripts/docs-generators/rust-crates.ts`.
//!
//! rustdoc JSON is nightly-only and its format changes between nightlies, so
//! this tool pins one nightly (`rust-toolchain.toml`, [`NIGHTLY`]) together
//! with the `rustdoc-types` release that reads its format, and refuses any
//! other format version.
//!
//! ```text
//! cua-rustdoc-json --workspace libs/cua --target-dir DIR [--out FILE] CRATE...
//! cua-rustdoc-json --from-json DIR [--out FILE] CRATE...   # reuse existing JSON
//! ```

use std::collections::{BTreeMap, HashSet};
use std::path::{Path as FsPath, PathBuf};
use std::process::Command;

use rustdoc_types::{
    Attribute, Crate, FORMAT_VERSION, FunctionHeader, FunctionSignature, GenericArg, GenericArgs,
    GenericBound, GenericParamDef, GenericParamDefKind, Generics, Id, Item, ItemEnum, Path,
    PreciseCapturingArg, StructKind, Term, TraitBoundModifier, Type, VariantKind, Visibility,
    WherePredicate,
};
use serde::Serialize;

/// The nightly whose rustdoc JSON matches `rustdoc-types` (keep in lockstep
/// with `rust-toolchain.toml` and `Cargo.toml`).
pub const NIGHTLY: &str = "nightly-2026-09-20";

#[derive(Serialize, Debug, PartialEq)]
pub struct Dump {
    pub tool: &'static str,
    pub toolchain: &'static str,
    pub format_version: u32,
    pub crates: Vec<CrateDoc>,
}

#[derive(Serialize, Debug, PartialEq)]
pub struct CrateDoc {
    pub name: String,
    pub lib: String,
    pub docs: String,
    pub items: Vec<ItemDoc>,
}

#[derive(Serialize, Debug, PartialEq, Default)]
pub struct ItemDoc {
    /// `module::path::Name` as a user imports it.
    pub path: String,
    pub module: String,
    pub name: String,
    pub kind: String,
    /// Source file relative to the crate root (`src/...`), for grouping.
    pub source: String,
    pub signature: String,
    pub docs: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deprecated: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub fields: Vec<Member>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub variants: Vec<Member>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub methods: Vec<Member>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub traits: Vec<String>,
}

#[derive(Serialize, Debug, PartialEq)]
pub struct Member {
    pub name: String,
    pub signature: String,
    pub docs: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deprecated: Option<String>,
}

/// Trait impls that are binding plumbing, not API (UniFFI converters, etc.).
const HIDDEN_TRAIT_CRATES: &[&str] = &["uniffi", "uniffi_core", "uniffi_macros"];

fn main() {
    if let Err(e) = run() {
        eprintln!("cua-rustdoc-json: {e}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let mut args = std::env::args().skip(1);
    let (mut workspace, mut target, mut from_json, mut out) = (None, None, None, None);
    let mut crates = Vec::new();
    while let Some(a) = args.next() {
        match a.as_str() {
            "--workspace" => workspace = args.next().map(PathBuf::from),
            "--target-dir" => target = args.next().map(PathBuf::from),
            "--from-json" => from_json = args.next().map(PathBuf::from),
            "--out" => out = args.next().map(PathBuf::from),
            "--print-toolchain" => {
                println!("{NIGHTLY}");
                return Ok(());
            }
            s if s.starts_with('-') => return Err(format!("unknown flag {s}")),
            _ => crates.push(a),
        }
    }
    if crates.is_empty() {
        return Err("no crates given".into());
    }
    let json_dir = match (from_json, workspace) {
        (Some(dir), _) => dir,
        (None, Some(ws)) => {
            let target = target.ok_or("--target-dir is required with --workspace")?;
            for krate in &crates {
                rustdoc(&ws, &target, krate)?;
            }
            target.join("doc")
        }
        (None, None) => return Err("pass --workspace DIR or --from-json DIR".into()),
    };
    let mut docs = Vec::new();
    for krate in &crates {
        let lib = krate.replace('-', "_");
        let file = json_dir.join(format!("{lib}.json"));
        let text = std::fs::read_to_string(&file)
            .map_err(|e| format!("reading {}: {e}", file.display()))?;
        docs.push(dump_crate(krate, &text)?);
    }
    let dump = Dump {
        tool: "cua-rustdoc-json",
        toolchain: NIGHTLY,
        format_version: FORMAT_VERSION,
        crates: docs,
    };
    let text = serde_json::to_string_pretty(&dump).map_err(|e| e.to_string())? + "\n";
    match out {
        Some(p) => std::fs::write(&p, text).map_err(|e| format!("writing {}: {e}", p.display())),
        None => {
            print!("{text}");
            Ok(())
        }
    }
}

/// `rustup run NIGHTLY cargo rustdoc -p CRATE --lib -- --output-format json`, with the
/// crate's default features (what a path/git dependency gets).
fn rustdoc(workspace: &FsPath, target: &FsPath, krate: &str) -> Result<(), String> {
    // `rustup run` so the pinned nightly wins over the workspace's own
    // rust-toolchain.toml (stable) for this call only, even when this tool
    // itself runs under `cargo run` (which sets CARGO to a toolchain binary).
    let status = Command::new("rustup")
        .current_dir(workspace)
        .args(["run", NIGHTLY, "cargo"])
        .args(["rustdoc", "-q", "-p", krate, "--lib", "--target-dir"])
        .arg(target)
        .args([
            "--",
            "-Zunstable-options",
            "--output-format",
            "json",
            "--cap-lints",
            "allow",
        ])
        .env_remove("RUSTUP_TOOLCHAIN")
        .env_remove("CARGO")
        .env_remove("RUSTC")
        .env_remove("RUSTDOC")
        .status()
        .map_err(|e| format!("running cargo rustdoc for {krate}: {e}"))?;
    if !status.success() {
        return Err(format!("cargo rustdoc -p {krate} failed ({status})"));
    }
    Ok(())
}

pub fn dump_crate(name: &str, json: &str) -> Result<CrateDoc, String> {
    // Check the version before a full parse so a mismatch reads clearly.
    #[derive(serde::Deserialize)]
    struct Version {
        format_version: u32,
    }
    let v: Version = serde_json::from_str(json).map_err(|e| format!("{name}: {e}"))?;
    if v.format_version != FORMAT_VERSION {
        return Err(format!(
            "{name}: rustdoc JSON format {} but rustdoc-types reads {FORMAT_VERSION}; \
             run with the pinned {NIGHTLY} (rust-toolchain.toml) or bump both together",
            v.format_version
        ));
    }
    let krate: Crate = serde_json::from_str(json).map_err(|e| format!("{name}: {e}"))?;
    let root = &krate.index[&krate.root];
    let lib = root.name.clone().unwrap_or_else(|| name.replace('-', "_"));
    let mut w = Walker {
        krate: &krate,
        items: Vec::new(),
        seen: HashSet::new(),
    };
    w.module(&krate.root, &lib, 0);
    // rustdoc lists glob re-exports last; sort so the dump reads by path.
    w.items.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(CrateDoc {
        name: name.into(),
        lib,
        docs: docs(root),
        items: w.items,
    })
}

struct Walker<'a> {
    krate: &'a Crate,
    items: Vec<ItemDoc>,
    seen: HashSet<(String, Id)>,
}

impl Walker<'_> {
    fn module(&mut self, id: &Id, path: &str, depth: usize) {
        if depth > 16 {
            return; // Glob cycles are legal Rust; bound the walk.
        }
        let Some(ItemEnum::Module(m)) = self.krate.index.get(id).map(|i| &i.inner) else {
            return;
        };
        for child in &m.items {
            let Some(item) = self.krate.index.get(child) else {
                continue;
            };
            if !is_public(item) || is_plumbing(item) {
                continue;
            }
            match &item.inner {
                ItemEnum::Use(u) => match u.id.as_ref().and_then(|t| self.krate.index.get(t)) {
                    Some(target) if u.is_glob => {
                        let target_id = target.id;
                        self.module(&target_id, path, depth + 1);
                    }
                    Some(target) => {
                        if is_plumbing(target) {
                            continue;
                        }
                        let target = target.clone();
                        self.item(&target, &u.name, path, depth, Some(item));
                    }
                    None => {
                        // A re-export of another crate's item (or glob).
                        let key = (path.to_string(), *child);
                        if self.seen.insert(key) {
                            let glob = if u.is_glob { "::*" } else { "" };
                            self.items.push(ItemDoc {
                                path: format!("{path}::{}", u.name),
                                module: path.into(),
                                name: u.name.clone(),
                                kind: "reexport".into(),
                                source: source(item),
                                signature: format!("pub use {}{glob};", u.source),
                                docs: docs(item),
                                ..Default::default()
                            });
                        }
                    }
                },
                _ => {
                    let name = item.name.clone().unwrap_or_default();
                    let item = item.clone();
                    self.item(&item, &name, path, depth, None);
                }
            }
        }
    }

    fn item(&mut self, item: &Item, name: &str, module: &str, depth: usize, via: Option<&Item>) {
        if !self.seen.insert((module.to_string(), item.id)) {
            return;
        }
        let path = format!("{module}::{name}");
        let k = self.krate;
        let mut doc = ItemDoc {
            path: path.clone(),
            module: module.into(),
            name: name.into(),
            source: source(item),
            docs: via
                .map(docs)
                .filter(|d| !d.is_empty())
                .unwrap_or_else(|| docs(item)),
            deprecated: deprecated(item),
            ..Default::default()
        };
        match &item.inner {
            ItemEnum::Module(_) => {
                doc.kind = "module".into();
                doc.signature = format!("pub mod {name}");
                self.items.push(doc);
                self.module(&item.id, &path, depth + 1);
                return;
            }
            ItemEnum::Struct(s) => {
                doc.kind = "struct".into();
                let g = generics(k, &s.generics);
                let wh = where_clause(k, &s.generics);
                doc.signature = match &s.kind {
                    StructKind::Unit => format!("pub struct {name}{g}{wh};"),
                    StructKind::Tuple(fields) => {
                        let f: Vec<String> = fields
                            .iter()
                            .map(|f| match f.and_then(|f| k.index.get(&f)) {
                                Some(Item {
                                    inner: ItemEnum::StructField(t),
                                    ..
                                }) => {
                                    format!("pub {}", ty(k, t))
                                }
                                _ => "_".into(),
                            })
                            .collect();
                        format!("pub struct {name}{g}({}){wh};", f.join(", "))
                    }
                    StructKind::Plain {
                        fields,
                        has_stripped_fields,
                    } => {
                        doc.fields = members(k, fields);
                        let body = if doc.fields.is_empty() && *has_stripped_fields {
                            " { /* private fields */ }"
                        } else if *has_stripped_fields {
                            " { /* public fields below, plus private fields */ }"
                        } else {
                            " { /* fields below */ }"
                        };
                        format!("pub struct {name}{g}{wh}{body}")
                    }
                };
                self.impls(&mut doc, &s.impls);
            }
            ItemEnum::Enum(e) => {
                doc.kind = "enum".into();
                doc.signature = format!(
                    "pub enum {name}{}{}",
                    generics(k, &e.generics),
                    where_clause(k, &e.generics)
                );
                doc.variants = e
                    .variants
                    .iter()
                    .filter_map(|v| k.index.get(v))
                    .filter_map(|v| {
                        let ItemEnum::Variant(var) = &v.inner else {
                            return None;
                        };
                        let vname = v.name.clone().unwrap_or_default();
                        let sig = match &var.kind {
                            VariantKind::Plain => vname.clone(),
                            VariantKind::Tuple(fs) => format!(
                                "{vname}({})",
                                fs.iter()
                                    .map(|f| match f.and_then(|f| k.index.get(&f)) {
                                        Some(Item {
                                            inner: ItemEnum::StructField(t),
                                            ..
                                        }) => {
                                            ty(k, t)
                                        }
                                        _ => "_".into(),
                                    })
                                    .collect::<Vec<_>>()
                                    .join(", ")
                            ),
                            VariantKind::Struct { fields, .. } => format!(
                                "{vname} {{ {} }}",
                                members(k, fields)
                                    .iter()
                                    .map(|m| m.signature.clone())
                                    .collect::<Vec<_>>()
                                    .join(", ")
                            ),
                        };
                        let sig = match &var.discriminant {
                            Some(d) => format!("{sig} = {}", d.expr),
                            None => sig,
                        };
                        Some(Member {
                            name: vname,
                            signature: sig,
                            docs: docs(v),
                            deprecated: deprecated(v),
                        })
                    })
                    .collect();
                if e.has_stripped_variants {
                    doc.signature.push_str(" /* some variants hidden */");
                }
                self.impls(&mut doc, &e.impls);
            }
            ItemEnum::Union(u) => {
                doc.kind = "union".into();
                doc.signature = format!("pub union {name}{}", generics(k, &u.generics));
                doc.fields = members(k, &u.fields);
                self.impls(&mut doc, &u.impls);
            }
            ItemEnum::Function(f) => {
                doc.kind = "function".into();
                doc.signature = fn_sig(k, name, &f.header, &f.generics, &f.sig);
            }
            ItemEnum::Trait(t) => {
                doc.kind = "trait".into();
                let bounds = if t.bounds.is_empty() {
                    String::new()
                } else {
                    format!(": {}", bounds_str(k, &t.bounds))
                };
                doc.signature = format!(
                    "pub {}{}trait {name}{}{bounds}{}",
                    if t.is_unsafe { "unsafe " } else { "" },
                    if t.is_auto { "auto " } else { "" },
                    generics(k, &t.generics),
                    where_clause(k, &t.generics)
                );
                doc.methods = t
                    .items
                    .iter()
                    .filter_map(|i| k.index.get(i))
                    .filter_map(|i| assoc_member(k, i))
                    .collect();
            }
            ItemEnum::TypeAlias(t) => {
                doc.kind = "type".into();
                doc.signature = format!(
                    "pub type {name}{}{} = {};",
                    generics(k, &t.generics),
                    where_clause(k, &t.generics),
                    ty(k, &t.type_)
                );
            }
            ItemEnum::Constant { type_, const_ } => {
                doc.kind = "constant".into();
                doc.signature = format!("pub const {name}: {} = {};", ty(k, type_), const_.expr);
            }
            ItemEnum::Static(s) => {
                doc.kind = "static".into();
                doc.signature = format!(
                    "pub static {}{name}: {};",
                    if s.is_mutable { "mut " } else { "" },
                    ty(k, &s.type_)
                );
            }
            ItemEnum::Macro(_) => {
                doc.kind = "macro".into();
                doc.signature = format!("macro_rules! {name}");
            }
            _ => return,
        }
        self.items.push(doc);
    }

    fn impls(&self, doc: &mut ItemDoc, impls: &[Id]) {
        let k = self.krate;
        let mut traits = BTreeMap::new();
        for id in impls {
            let Some(Item {
                inner: ItemEnum::Impl(imp),
                ..
            }) = k.index.get(id)
            else {
                continue;
            };
            if imp.is_synthetic || imp.blanket_impl.is_some() {
                continue;
            }
            match &imp.trait_ {
                None => {
                    for m in imp.items.iter().filter_map(|i| k.index.get(i)) {
                        if is_public(m)
                            && let Some(member) = assoc_member(k, m)
                        {
                            doc.methods.push(member);
                        }
                    }
                }
                Some(tr) => {
                    if is_hidden_trait(k, tr) {
                        continue;
                    }
                    let neg = if imp.is_negative { "!" } else { "" };
                    let name = format!("{neg}{}", path_str(k, tr));
                    traits.insert(name, ());
                }
            }
        }
        doc.traits = traits.into_keys().collect();
    }
}

fn is_public(item: &Item) -> bool {
    matches!(item.visibility, Visibility::Public | Visibility::Default)
}

/// Generated FFI surface (UniFFI scaffolding, `#[no_mangle]` exports).
fn is_plumbing(item: &Item) -> bool {
    let name = item.name.as_deref().unwrap_or("");
    name.starts_with("uniffi_")
        || name.starts_with("ffi_")
        || name.starts_with("UniFfi")
        || name.starts_with("UNIFFI_")
        || item
            .attrs
            .iter()
            .any(|a| matches!(a, Attribute::NoMangle | Attribute::ExportName(_)))
}

fn is_hidden_trait(k: &Crate, tr: &Path) -> bool {
    let Some(summary) = k.paths.get(&tr.id) else {
        return false;
    };
    let krate = k
        .external_crates
        .get(&summary.crate_id)
        .map(|c| c.name.as_str());
    krate.is_some_and(|c| HIDDEN_TRAIT_CRATES.contains(&c))
        || summary
            .path
            .first()
            .is_some_and(|p| HIDDEN_TRAIT_CRATES.contains(&p.as_str()))
}

fn docs(item: &Item) -> String {
    item.docs.clone().unwrap_or_default().trim().to_string()
}

fn deprecated(item: &Item) -> Option<String> {
    item.deprecation.as_ref().map(|d| {
        let mut s = String::from("deprecated");
        if let Some(since) = &d.since {
            s.push_str(&format!(" since {since}"));
        }
        if let Some(note) = &d.note {
            s.push_str(&format!(": {note}"));
        }
        s
    })
}

/// `src/...` relative to the crate root, never an absolute path.
fn source(item: &Item) -> String {
    let Some(span) = &item.span else {
        return String::new();
    };
    let p = span.filename.to_string_lossy().replace('\\', "/");
    match p.rfind("/src/") {
        Some(i) => p[i + 1..].to_string(),
        None if p.starts_with("src/") => p,
        None => String::new(),
    }
}

fn members(k: &Crate, ids: &[Id]) -> Vec<Member> {
    ids.iter()
        .filter_map(|f| k.index.get(f))
        .filter_map(|f| match &f.inner {
            ItemEnum::StructField(t) => {
                let name = f.name.clone().unwrap_or_default();
                Some(Member {
                    signature: format!("{name}: {}", ty(k, t)),
                    name,
                    docs: docs(f),
                    deprecated: deprecated(f),
                })
            }
            _ => None,
        })
        .collect()
}

fn assoc_member(k: &Crate, item: &Item) -> Option<Member> {
    let name = item.name.clone()?;
    let signature = match &item.inner {
        ItemEnum::Function(f) => fn_sig(k, &name, &f.header, &f.generics, &f.sig),
        ItemEnum::AssocConst { type_, value, .. } => match value {
            Some(v) => format!("const {name}: {} = {v};", ty(k, type_)),
            None => format!("const {name}: {};", ty(k, type_)),
        },
        ItemEnum::AssocType {
            generics: g,
            bounds,
            type_,
            ..
        } => {
            let b = if bounds.is_empty() {
                String::new()
            } else {
                format!(": {}", bounds_str(k, bounds))
            };
            let d = type_
                .as_ref()
                .map(|t| format!(" = {}", ty(k, t)))
                .unwrap_or_default();
            format!("type {name}{}{b}{d};", generics(k, g))
        }
        _ => return None,
    };
    Some(Member {
        name,
        signature,
        docs: docs(item),
        deprecated: deprecated(item),
    })
}

pub fn fn_sig(
    k: &Crate,
    name: &str,
    h: &FunctionHeader,
    g: &Generics,
    sig: &FunctionSignature,
) -> String {
    let mut s = String::from("pub ");
    if h.is_const {
        s.push_str("const ");
    }
    if h.is_async {
        s.push_str("async ");
    }
    if h.is_unsafe {
        s.push_str("unsafe ");
    }
    s.push_str("fn ");
    s.push_str(name);
    s.push_str(&generics(k, g));
    let args: Vec<String> = sig
        .inputs
        .iter()
        .map(|(n, t)| self_arg(n, t).unwrap_or_else(|| format!("{n}: {}", ty(k, t))))
        .collect();
    s.push('(');
    s.push_str(&args.join(", "));
    if sig.is_c_variadic {
        s.push_str(", ...");
    }
    s.push(')');
    if let Some(out) = &sig.output {
        s.push_str(" -> ");
        s.push_str(&ty(k, out));
    }
    s.push_str(&where_clause(k, g));
    s
}

fn self_arg(n: &str, t: &Type) -> Option<String> {
    if n != "self" {
        return None;
    }
    Some(match t {
        Type::Generic(g) if g == "Self" => "self".into(),
        Type::BorrowedRef {
            lifetime,
            is_mutable,
            type_,
        } if matches!(&**type_, Type::Generic(g) if g == "Self") => {
            format!(
                "&{}{}self",
                lifetime
                    .as_ref()
                    .map(|l| format!("{l} "))
                    .unwrap_or_default(),
                if *is_mutable { "mut " } else { "" }
            )
        }
        _ => return None,
    })
}

fn generics(k: &Crate, g: &Generics) -> String {
    let params: Vec<String> = g
        .params
        .iter()
        .filter(|p| {
            !matches!(
                p.kind,
                GenericParamDefKind::Type {
                    is_synthetic: true,
                    ..
                }
            )
        })
        .map(|p| param(k, p))
        .collect();
    if params.is_empty() {
        String::new()
    } else {
        format!("<{}>", params.join(", "))
    }
}

fn param(k: &Crate, p: &GenericParamDef) -> String {
    match &p.kind {
        GenericParamDefKind::Lifetime { outlives } if outlives.is_empty() => p.name.clone(),
        GenericParamDefKind::Lifetime { outlives } => {
            format!("{}: {}", p.name, outlives.join(" + "))
        }
        GenericParamDefKind::Type {
            bounds, default, ..
        } => {
            let mut s = p.name.clone();
            if !bounds.is_empty() {
                s.push_str(": ");
                s.push_str(&bounds_str(k, bounds));
            }
            if let Some(d) = default {
                s.push_str(" = ");
                s.push_str(&ty(k, d));
            }
            s
        }
        GenericParamDefKind::Const { type_, default } => {
            let d = default
                .as_ref()
                .map(|d| format!(" = {d}"))
                .unwrap_or_default();
            format!("const {}: {}{d}", p.name, ty(k, type_))
        }
    }
}

fn where_clause(k: &Crate, g: &Generics) -> String {
    let preds: Vec<String> = g
        .where_predicates
        .iter()
        .map(|p| match p {
            WherePredicate::BoundPredicate {
                type_,
                bounds,
                generic_params,
            } => {
                let hr = if generic_params.is_empty() {
                    String::new()
                } else {
                    let ps: Vec<String> = generic_params.iter().map(|p| param(k, p)).collect();
                    format!("for<{}> ", ps.join(", "))
                };
                format!("{hr}{}: {}", ty(k, type_), bounds_str(k, bounds))
            }
            WherePredicate::LifetimePredicate { lifetime, outlives } => {
                format!("{lifetime}: {}", outlives.join(" + "))
            }
            WherePredicate::EqPredicate { lhs, rhs } => {
                format!("{} == {}", ty(k, lhs), term(k, rhs))
            }
        })
        .collect();
    if preds.is_empty() {
        String::new()
    } else {
        format!(" where {}", preds.join(", "))
    }
}

fn bounds_str(k: &Crate, bounds: &[GenericBound]) -> String {
    bounds
        .iter()
        .map(|b| match b {
            GenericBound::TraitBound {
                trait_,
                generic_params,
                modifier,
            } => {
                let hr = if generic_params.is_empty() {
                    String::new()
                } else {
                    let ps: Vec<String> = generic_params.iter().map(|p| param(k, p)).collect();
                    format!("for<{}> ", ps.join(", "))
                };
                let m = match modifier {
                    TraitBoundModifier::None => "",
                    TraitBoundModifier::Maybe => "?",
                    TraitBoundModifier::MaybeConst => "~const ",
                };
                format!("{hr}{m}{}", path_str(k, trait_))
            }
            GenericBound::Outlives(l) => l.clone(),
            GenericBound::Use(args) => format!(
                "use<{}>",
                args.iter()
                    .map(|a| match a {
                        PreciseCapturingArg::Lifetime(l) => l.clone(),
                        PreciseCapturingArg::Param(p) => p.clone(),
                    })
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        })
        .collect::<Vec<_>>()
        .join(" + ")
}

fn term(k: &Crate, t: &Term) -> String {
    match t {
        Term::Type(t) => ty(k, t),
        Term::Constant(c) => c.expr.clone(),
    }
}

fn path_str(k: &Crate, p: &Path) -> String {
    // The path as written, trimmed to its last segment for readability.
    let base = p.path.rsplit("::").next().unwrap_or(&p.path).to_string();
    match p.args.as_deref() {
        Some(a) => format!("{base}{}", generic_args(k, a)),
        None => base,
    }
}

fn generic_args(k: &Crate, a: &GenericArgs) -> String {
    match a {
        GenericArgs::AngleBracketed { args, constraints } => {
            let mut parts: Vec<String> = args
                .iter()
                .map(|a| match a {
                    GenericArg::Lifetime(l) => l.clone(),
                    GenericArg::Type(t) => ty(k, t),
                    GenericArg::Const(c) => c.expr.clone(),
                    GenericArg::Infer => "_".into(),
                })
                .collect();
            for c in constraints {
                let args = c
                    .args
                    .as_deref()
                    .map(|a| generic_args(k, a))
                    .unwrap_or_default();
                parts.push(match &c.binding {
                    rustdoc_types::AssocItemConstraintKind::Equality(t) => {
                        format!("{}{args} = {}", c.name, term(k, t))
                    }
                    rustdoc_types::AssocItemConstraintKind::Constraint(b) => {
                        format!("{}{args}: {}", c.name, bounds_str(k, b))
                    }
                });
            }
            if parts.is_empty() {
                String::new()
            } else {
                format!("<{}>", parts.join(", "))
            }
        }
        GenericArgs::Parenthesized { inputs, output } => {
            let ins: Vec<String> = inputs.iter().map(|t| ty(k, t)).collect();
            let out = output
                .as_ref()
                .map(|o| format!(" -> {}", ty(k, o)))
                .unwrap_or_default();
            format!("({}){out}", ins.join(", "))
        }
        GenericArgs::ReturnTypeNotation => "(..)".into(),
    }
}

pub fn ty(k: &Crate, t: &Type) -> String {
    match t {
        Type::ResolvedPath(p) => path_str(k, p),
        Type::DynTrait(d) => {
            let mut parts: Vec<String> = d
                .traits
                .iter()
                .map(|pt| {
                    if pt.generic_params.is_empty() {
                        path_str(k, &pt.trait_)
                    } else {
                        let ps: Vec<String> =
                            pt.generic_params.iter().map(|p| param(k, p)).collect();
                        format!("for<{}> {}", ps.join(", "), path_str(k, &pt.trait_))
                    }
                })
                .collect();
            if let Some(l) = &d.lifetime {
                parts.push(l.clone());
            }
            format!("dyn {}", parts.join(" + "))
        }
        Type::Generic(g) => g.clone(),
        Type::Primitive(p) => p.clone(),
        Type::FunctionPointer(f) => {
            let args: Vec<String> = f.sig.inputs.iter().map(|(_, t)| ty(k, t)).collect();
            let out = f
                .sig
                .output
                .as_ref()
                .map(|o| format!(" -> {}", ty(k, o)))
                .unwrap_or_default();
            format!(
                "{}fn({}){out}",
                if f.header.is_unsafe { "unsafe " } else { "" },
                args.join(", ")
            )
        }
        Type::Tuple(ts) if ts.len() == 1 => format!("({},)", ty(k, &ts[0])),
        Type::Tuple(ts) => format!(
            "({})",
            ts.iter().map(|t| ty(k, t)).collect::<Vec<_>>().join(", ")
        ),
        Type::Slice(t) => format!("[{}]", ty(k, t)),
        Type::Array { type_, len } => format!("[{}; {len}]", ty(k, type_)),
        Type::Pat { type_, .. } => ty(k, type_),
        Type::ImplTrait(b) => format!("impl {}", bounds_str(k, b)),
        Type::Infer => "_".into(),
        Type::RawPointer { is_mutable, type_ } => {
            format!(
                "*{} {}",
                if *is_mutable { "mut" } else { "const" },
                ty(k, type_)
            )
        }
        Type::BorrowedRef {
            lifetime,
            is_mutable,
            type_,
        } => format!(
            "&{}{}{}",
            lifetime
                .as_ref()
                .map(|l| format!("{l} "))
                .unwrap_or_default(),
            if *is_mutable { "mut " } else { "" },
            ty(k, type_)
        ),
        Type::QualifiedPath {
            name,
            args,
            self_type,
            trait_,
        } => {
            let a = args
                .as_deref()
                .map(|a| generic_args(k, a))
                .unwrap_or_default();
            match trait_ {
                Some(tr) if !tr.path.is_empty() => {
                    format!("<{} as {}>::{name}{a}", ty(k, self_type), path_str(k, tr))
                }
                _ => format!("{}::{name}{a}", ty(k, self_type)),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIB: &str = r#"
//! Demo crate.
mod inner {
    /// A handle.
    #[derive(Clone, Debug)]
    pub struct Handle { /// The id.
        pub id: u32, secret: u8 }
    impl Handle {
        /// Makes one.
        pub async fn open(name: &str, tags: Vec<String>) -> Result<Self, Error> { let _ = (name, tags); Ok(Handle { id: 1, secret: 0 }) }
        fn hidden(&self) {}
        /// Borrow it.
        pub fn get<'a>(&'a self) -> &'a u32 { let _ = self.secret; &self.id }
    }
    /// Errors.
    #[derive(Debug)]
    pub enum Error { /// Nope.
        NotFound(String), Other { code: i32 } }
}
pub use inner::*;
/// Things.
pub mod things {
    /// A trait.
    pub trait Thing: Send { /// Run.
        fn run(&mut self, n: Option<u8>) -> impl Iterator<Item = u8>; }
    /// Old name.
    #[deprecated(since = "0.2.0", note = "use Handle")]
    pub type Old = super::Handle;
}
/// The version.
pub const VERSION: &str = "1";
#[unsafe(no_mangle)]
pub extern "C" fn ffi_demo() {}
"#;

    fn demo_json() -> String {
        let dir =
            std::env::temp_dir().join(format!("cua-rustdoc-json-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("demo.rs");
        std::fs::write(&src, LIB).unwrap();
        let out = Command::new("rustdoc")
            .args([
                "--edition",
                "2024",
                "--crate-type",
                "lib",
                "-Zunstable-options",
                "--output-format",
                "json",
                "-o",
            ])
            .arg(&dir)
            .arg(&src)
            .env_remove("RUSTDOC")
            .output()
            .expect("run rustdoc (the pinned nightly from rust-toolchain.toml)");
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let json = std::fs::read_to_string(dir.join("demo.json")).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        json
    }

    #[test]
    fn dumps_public_api_through_glob_reexports() {
        let doc = dump_crate("demo", &demo_json()).unwrap();
        assert_eq!(doc.docs, "Demo crate.");
        let names: Vec<&str> = doc.items.iter().map(|i| i.path.as_str()).collect();
        assert_eq!(
            names,
            [
                "demo::Error",
                "demo::Handle",
                "demo::VERSION",
                "demo::things",
                "demo::things::Old",
                "demo::things::Thing"
            ]
        );
        let h = &doc.items[1];
        assert_eq!(h.kind, "struct");
        assert_eq!(h.docs, "A handle.");
        assert_eq!(h.traits, ["Clone", "Debug"]);
        assert_eq!(h.fields.len(), 1);
        assert_eq!(h.fields[0].signature, "id: u32");
        let sigs: Vec<&str> = h.methods.iter().map(|m| m.signature.as_str()).collect();
        assert_eq!(
            sigs,
            [
                "pub async fn open(name: &str, tags: Vec<String>) -> Result<Self, Error>",
                "pub fn get<'a>(&'a self) -> &'a u32"
            ]
        );
        let e = &doc.items[0];
        let vs: Vec<&str> = e.variants.iter().map(|v| v.signature.as_str()).collect();
        assert_eq!(vs, ["NotFound(String)", "Other { code: i32 }"]);
        let t = &doc.items[5];
        assert_eq!(t.signature, "pub trait Thing: Send");
        assert_eq!(
            t.methods[0].signature,
            "pub fn run(&mut self, n: Option<u8>) -> impl Iterator<Item = u8>"
        );
        let old = &doc.items[4];
        assert_eq!(old.signature, "pub type Old = Handle;");
        assert_eq!(
            old.deprecated.as_deref(),
            Some("deprecated since 0.2.0: use Handle")
        );
        assert_eq!(doc.items[2].signature, "pub const VERSION: &str = \"1\";");
        assert!(doc.items.iter().all(|i| !i.source.starts_with('/')));
    }

    #[test]
    fn rejects_other_format_versions() {
        let err = dump_crate("demo", r#"{"format_version": 1}"#).unwrap_err();
        assert!(err.contains(NIGHTLY), "{err}");
    }

    #[test]
    fn pinned_toolchain_matches_rust_toolchain_toml() {
        let toml = include_str!("../rust-toolchain.toml");
        assert!(
            toml.contains(&format!("channel = \"{NIGHTLY}\"")),
            "rust-toolchain.toml is out of lockstep"
        );
    }
}
