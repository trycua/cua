//! Structured, format-preserving edits of one named entry inside an agent's
//! MCP config: JSON/JSONC (comments, trailing commas and loose JSON5-style
//! keys tolerated and kept) through jsonc-parser's CST, TOML through
//! toml_edit. Everything outside the edited entry is left byte-for-byte
//! as it was. YAML (Goose, Hermes) is spliced line by line when the entry
//! sits under a top-level block mapping, and the result must parse back to
//! exactly the expected document; any other layout is round-tripped through
//! serde, and only when the file has no comments that would be lost.

use crate::{Error, Result};
use jsonc_parser::cst::{CstInputValue, CstObject, CstRootNode};
use serde_json::Value;
use std::path::Path;

/// A config file syntax.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Format {
    /// JSON, JSONC or JSON5-ish (comments and trailing commas kept).
    Json,
    /// TOML.
    Toml,
    /// YAML (comments kept for entries under a top-level block mapping).
    Yaml,
}

impl Format {
    /// Human name.
    pub fn name(self) -> &'static str {
        match self {
            Format::Json => "JSON",
            Format::Toml => "TOML",
            Format::Yaml => "YAML",
        }
    }
}

fn malformed(file: &Path, format: Format, msg: impl std::fmt::Display) -> Error {
    Error::Malformed {
        path: file.to_path_buf(),
        detail: format!("{} parse error: {msg}", format.name()),
    }
}

/// The entry `name` under the object at `path` (for example
/// `["mcpServers"]`), as JSON. `Ok(None)` when absent.
pub fn get(
    format: Format,
    file: &Path,
    text: &str,
    path: &[&str],
    name: &str,
) -> Result<Option<Value>> {
    match format {
        Format::Json => {
            let root = parse_json(file, text)?;
            let Some(obj) = json_container(file, &root, path, false)? else {
                return Ok(None);
            };
            Ok(obj
                .get(name)
                .and_then(|p| p.value())
                .and_then(|v| v.to_serde_value()))
        }
        Format::Toml => {
            let doc = parse_toml(file, text)?;
            let Some(t) = toml_container_ref(file, &doc, path)? else {
                return Ok(None);
            };
            Ok(t.get(name).map(toml_item_to_json))
        }
        Format::Yaml => {
            let root = parse_yaml(file, text)?;
            Ok(yaml_container(file, &root, path)?.and_then(|m| m.get(name).cloned()))
        }
    }
}

/// All entry names under `path`.
#[cfg_attr(not(test), allow(dead_code))]
pub fn names(format: Format, file: &Path, text: &str, path: &[&str]) -> Result<Vec<String>> {
    match format {
        Format::Json => {
            let root = parse_json(file, text)?;
            Ok(json_container(file, &root, path, false)?
                .map(|o| {
                    o.properties()
                        .iter()
                        .filter_map(|p| p.decoded_name())
                        .collect()
                })
                .unwrap_or_default())
        }
        Format::Toml => {
            let doc = parse_toml(file, text)?;
            Ok(toml_container_ref(file, &doc, path)?
                .map(|t| t.iter().map(|(k, _)| k.to_string()).collect())
                .unwrap_or_default())
        }
        Format::Yaml => {
            let root = parse_yaml(file, text)?;
            Ok(yaml_container(file, &root, path)?
                .map(|m| m.keys().cloned().collect())
                .unwrap_or_default())
        }
    }
}

/// An entry's fields in the order they are written.
pub type Fields = Vec<(String, Value)>;

/// Fields as a JSON object (order-insensitive comparisons).
pub fn to_value(fields: &Fields) -> Value {
    Value::Object(fields.iter().cloned().collect())
}

/// Sets entry `name` under `path` to `fields` (creating the parents), and
/// returns the new text. Every other entry, comment and blank line stays.
pub fn upsert(
    format: Format,
    file: &Path,
    text: &str,
    path: &[&str],
    name: &str,
    fields: &Fields,
) -> Result<String> {
    let value = &to_value(fields);
    let out = match format {
        Format::Json => {
            let root = parse_json(file, text)?;
            let obj = json_container(file, &root, path, true)?.expect("created");
            match obj.get(name) {
                Some(p) => p.set_value(fields_to_cst(fields)),
                None => {
                    obj.append(name, fields_to_cst(fields));
                }
            }
            let mut s = root.to_string();
            if text.trim().is_empty() && !s.ends_with('\n') {
                s.push('\n');
            }
            s
        }
        Format::Toml => {
            let mut doc = parse_toml(file, text)?;
            let t = toml_container_mut(file, &mut doc, path)?;
            t.insert(name, fields_to_toml(fields));
            doc.to_string()
        }
        Format::Yaml => {
            let mut root = parse_yaml(file, text)?;
            yaml_container_mut(file, &mut root, path)?.insert(name.to_string(), value.clone());
            match yaml_splice_upsert(text, path, name, fields)
                .filter(|out| parse_yaml(file, out).is_ok_and(|got| got == root))
            {
                Some(out) => out,
                None => {
                    refuse_yaml_comments(file, text)?;
                    dump_yaml(file, &root, fields, path, name)?
                }
            }
        }
    };
    // Belt and braces: the edit must read back as exactly `value`.
    let back = get(format, file, &out, path, name)?;
    if back.as_ref() != Some(value) {
        return Err(Error::Internal(format!(
            "edit of {} did not round-trip (got {back:?})",
            file.display()
        )));
    }
    Ok(out)
}

/// Removes entry `name` under `path`. Returns the new text, or `None` when
/// there was nothing to remove. Empty parents are left in place.
pub fn remove(
    format: Format,
    file: &Path,
    text: &str,
    path: &[&str],
    name: &str,
) -> Result<Option<String>> {
    match format {
        Format::Json => {
            let root = parse_json(file, text)?;
            let Some(obj) = json_container(file, &root, path, false)? else {
                return Ok(None);
            };
            let Some(p) = obj.get(name) else {
                return Ok(None);
            };
            p.remove();
            Ok(Some(root.to_string()))
        }
        Format::Toml => {
            let mut doc = parse_toml(file, text)?;
            if toml_container_ref(file, &doc, path)?.is_none() {
                return Ok(None);
            }
            let t = toml_container_mut(file, &mut doc, path)?;
            if t.remove(name).is_none() {
                return Ok(None);
            }
            Ok(Some(doc.to_string()))
        }
        Format::Yaml => {
            let mut root = parse_yaml(file, text)?;
            if yaml_container(file, &root, path)?.is_none_or(|m| !m.contains_key(name)) {
                return Ok(None);
            }
            yaml_container_mut(file, &mut root, path)?.remove(name);
            let want = without_empty_top(root.clone(), path);
            if let Some(out) = yaml_splice_remove(text, path, name).filter(|out| {
                parse_yaml(file, out).is_ok_and(|got| without_empty_top(got, path) == want)
            }) {
                return Ok(Some(out));
            }
            refuse_yaml_comments(file, text)?;
            Ok(Some(dump_yaml(file, &root, &[], path, name)?))
        }
    }
}

/// Checks that `text` parses and can be edited (used before touching a
/// file).
pub fn validate(format: Format, file: &Path, text: &str) -> Result<()> {
    match format {
        Format::Json => {
            let root = parse_json(file, text)?;
            if root.value().is_some() && root.object_value().is_none() {
                return Err(Error::Malformed {
                    path: file.to_path_buf(),
                    detail: "the top level is not a JSON object".into(),
                });
            }
            Ok(())
        }
        Format::Toml => parse_toml(file, text).map(|_| ()),
        // Comment loss is refused by the edit itself, which only falls back
        // to a serde round trip for layouts the splice does not handle.
        Format::Yaml => parse_yaml(file, text).map(|_| ()),
    }
}

/// `fields` as compact JSON in their order (for agent CLIs that take JSON).
pub fn fields_json(fields: &Fields) -> String {
    let parts: Vec<String> = fields
        .iter()
        .map(|(k, v)| format!("{}:{}", Value::String(k.clone()), v))
        .collect();
    format!("{{{}}}", parts.join(","))
}

// ------------------------------------------------------------------ YAML

type JsonMap = serde_json::Map<String, Value>;

/// YAML goes through serde, which drops comments: refuse rather than lose
/// them (the caller prints the snippet to add by hand).
fn refuse_yaml_comments(file: &Path, text: &str) -> Result<()> {
    let commented = text.lines().any(|l| {
        let t = l.trim_start();
        t.starts_with('#') || l.contains(" #")
    });
    if commented {
        return Err(Error::Malformed {
            path: file.to_path_buf(),
            detail: "the YAML file has comments that an automatic edit would drop; add the entry by hand".into(),
        });
    }
    Ok(())
}

fn parse_yaml(file: &Path, text: &str) -> Result<JsonMap> {
    if text.trim().is_empty() {
        return Ok(JsonMap::new());
    }
    let v: serde_yaml_ng::Value =
        serde_yaml_ng::from_str(text).map_err(|e| malformed(file, Format::Yaml, e))?;
    match serde_json::to_value(v).map_err(|e| malformed(file, Format::Yaml, e))? {
        Value::Object(m) => Ok(m),
        Value::Null => Ok(JsonMap::new()),
        _ => Err(Error::Malformed {
            path: file.to_path_buf(),
            detail: "the top level is not a YAML mapping".into(),
        }),
    }
}

fn yaml_container<'a>(
    file: &Path,
    root: &'a JsonMap,
    path: &[&str],
) -> Result<Option<&'a JsonMap>> {
    let mut m = root;
    for (i, seg) in path.iter().enumerate() {
        match m.get(*seg) {
            None | Some(Value::Null) => return Ok(None),
            Some(Value::Object(o)) => m = o,
            Some(_) => {
                return Err(Error::Malformed {
                    path: file.to_path_buf(),
                    detail: format!("\"{}\" is not a mapping", path[..=i].join(".")),
                });
            }
        }
    }
    Ok(Some(m))
}

fn yaml_container_mut<'a>(
    file: &Path,
    root: &'a mut JsonMap,
    path: &[&str],
) -> Result<&'a mut JsonMap> {
    let mut m = root;
    for (i, seg) in path.iter().enumerate() {
        let e = m
            .entry(seg.to_string())
            .or_insert_with(|| Value::Object(JsonMap::new()));
        if e.is_null() {
            *e = Value::Object(JsonMap::new());
        }
        m = match e {
            Value::Object(o) => o,
            _ => {
                return Err(Error::Malformed {
                    path: file.to_path_buf(),
                    detail: format!("\"{}\" is not a mapping", path[..=i].join(".")),
                });
            }
        };
    }
    Ok(m)
}

/// Serializes, writing the edited entry's fields in `fields` order.
fn dump_yaml(
    file: &Path,
    root: &JsonMap,
    fields: &[(String, Value)],
    path: &[&str],
    name: &str,
) -> Result<String> {
    // serde_json maps are sorted; rebuild as an ordered YAML mapping so the
    // entry reads naturally (`type`, `name`, `cmd`, `args`, ...).
    fn to_yaml(v: &Value) -> serde_yaml_ng::Value {
        serde_yaml_ng::to_value(v).unwrap_or(serde_yaml_ng::Value::Null)
    }
    let mut y = to_yaml(&Value::Object(root.clone()));
    if !fields.is_empty() {
        let mut cur = &mut y;
        for seg in path {
            cur = cur
                .get_mut(*seg)
                .ok_or_else(|| Error::Internal("yaml path vanished".into()))?;
        }
        let mut ordered = serde_yaml_ng::Mapping::new();
        for (k, v) in fields {
            ordered.insert(serde_yaml_ng::Value::from(k.as_str()), to_yaml(v));
        }
        if let Some(m) = cur.as_mapping_mut() {
            m.insert(
                serde_yaml_ng::Value::from(name),
                serde_yaml_ng::Value::Mapping(ordered),
            );
        }
    }
    serde_yaml_ng::to_string(&y).map_err(|e| malformed(file, Format::Yaml, e))
}

// ------------------------------------------------------------- YAML splice
//
// A line-level edit of one entry under a top-level block mapping
// (`mcp_servers:` / `extensions:` followed by indented entries), so the
// rest of the file (comments, blank lines, quoting, key order) stays byte
// for byte. Each returns `None` for layouts it does not handle; callers
// also check that the result parses to exactly the expected document.

/// The `key` of a plain `key: rest` line (no indentation), and the rest.
fn yaml_plain_key(line: &str) -> Option<(&str, &str)> {
    let colon = line.find(':')?;
    let (key, rest) = (&line[..colon], &line[colon + 1..]);
    let plain = !key.is_empty()
        && key
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'));
    (plain && (rest.is_empty() || rest.starts_with([' ', '\t']))).then_some((key, rest))
}

fn yaml_line_body(l: &str) -> &str {
    l.trim_end_matches(['\n', '\r'])
}

fn yaml_indent(l: &str) -> usize {
    l.len() - l.trim_start_matches(' ').len()
}

/// Neither blank nor a comment.
fn yaml_has_content(l: &str) -> bool {
    let t = yaml_line_body(l).trim();
    !t.is_empty() && !t.starts_with('#')
}

/// The top-level `key:` line, and whether its value is a block that
/// follows on the next lines (`key:` or `key:  # comment`) or an inline
/// empty value (`{}`, `null`, `~`). Other inline values are not handled.
fn yaml_top_key(lines: &[&str], key: &str) -> Option<Option<(usize, bool)>> {
    let mut found = None;
    for (i, l) in lines.iter().enumerate() {
        let body = yaml_line_body(l);
        if yaml_indent(body) != 0 || !yaml_has_content(body) {
            continue;
        }
        if body == "---" || body == "..." {
            return None;
        }
        let Some((k, rest)) = yaml_plain_key(body) else {
            continue;
        };
        if k != key {
            continue;
        }
        let v = rest.trim();
        let block = v.is_empty() || v.starts_with('#');
        if !block && !matches!(v, "{}" | "null" | "~") {
            return None;
        }
        if found.is_some() {
            return None;
        }
        found = Some((i, block));
    }
    Some(found)
}

/// The region of the block under the key at `top`: up to the next
/// top-level content line.
fn yaml_region_end(lines: &[&str], top: usize) -> usize {
    (top + 1..lines.len())
        .find(|&j| yaml_has_content(lines[j]) && yaml_indent(lines[j]) == 0)
        .unwrap_or(lines.len())
}

/// `(indent of the entries, entry span [start, end))` for `name` in the
/// region, or `(indent, None)` when absent. The span ends after the entry's
/// last content line, so trailing comments and blank lines stay.
fn yaml_find_entry(
    lines: &[&str],
    region: std::ops::Range<usize>,
    name: &str,
) -> Option<(usize, Option<(usize, usize)>)> {
    let Some(first) = region.clone().find(|&j| yaml_has_content(lines[j])) else {
        return Some((2, None));
    };
    let ind = yaml_indent(lines[first]);
    if ind == 0 || yaml_line_body(lines[first]).trim_start().starts_with('-') {
        return None;
    }
    let start = region.clone().find(|&j| {
        let b = yaml_line_body(lines[j]);
        yaml_has_content(b)
            && yaml_indent(b) == ind
            && yaml_plain_key(&b[ind..]).is_some_and(|(k, _)| k == name)
    });
    let Some(start) = start else {
        return Some((ind, None));
    };
    // The entry's last content line: before the next line at its indent.
    let last = (start + 1..region.end)
        .filter(|&j| yaml_has_content(lines[j]))
        .take_while(|&j| yaml_indent(lines[j]) > ind)
        .last()
        .unwrap_or(start);
    Some((ind, Some((start, last + 1))))
}

/// `name:` and its fields at `ind` spaces, block style.
fn yaml_render_entry(name: &str, fields: &Fields, ind: usize, eol: &str) -> Option<String> {
    yaml_plain_key(&format!("{name}:"))?;
    let pad = " ".repeat(ind);
    let mut out = format!("{pad}{name}:{eol}");
    for (k, v) in fields {
        yaml_plain_key(&format!("{k}:"))?;
        let nested = match v {
            Value::Array(a) => !a.is_empty(),
            Value::Object(o) => !o.is_empty(),
            _ => false,
        };
        let y = serde_yaml_ng::to_string(v).ok()?;
        if nested {
            out.push_str(&format!("{pad}  {k}:{eol}"));
            for l in y.lines() {
                out.push_str(&format!("{pad}    {l}{eol}"));
            }
        } else {
            let y = y.trim_end();
            if y.contains('\n') {
                return None;
            }
            out.push_str(&format!("{pad}  {k}: {y}{eol}"));
        }
    }
    Some(out)
}

fn yaml_eol(text: &str) -> &'static str {
    if text.contains("\r\n") { "\r\n" } else { "\n" }
}

fn yaml_splice_upsert(text: &str, path: &[&str], name: &str, fields: &Fields) -> Option<String> {
    let [key] = path else { return None };
    let eol = yaml_eol(text);
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let mut out = String::with_capacity(text.len() + 256);
    match yaml_top_key(&lines, key)? {
        None => {
            out.push_str(text);
            if !text.is_empty() && !text.ends_with('\n') {
                out.push_str(eol);
            }
            out.push_str(&format!("{key}:{eol}"));
            out.push_str(&yaml_render_entry(name, fields, 2, eol)?);
        }
        Some((top, false)) => {
            lines[..top].iter().for_each(|l| out.push_str(l));
            out.push_str(&format!("{key}:{eol}"));
            out.push_str(&yaml_render_entry(name, fields, 2, eol)?);
            lines[top + 1..].iter().for_each(|l| out.push_str(l));
        }
        Some((top, true)) => {
            let end = yaml_region_end(&lines, top);
            let (ind, span) = yaml_find_entry(&lines, top + 1..end, name)?;
            let (from, to) = match span {
                Some(s) => s,
                None => {
                    // After the region's last content line (or the key).
                    let at = (top + 1..end)
                        .rev()
                        .find(|&j| yaml_has_content(lines[j]))
                        .map_or(top + 1, |j| j + 1);
                    (at, at)
                }
            };
            lines[..from].iter().for_each(|l| out.push_str(l));
            if !out.is_empty() && !out.ends_with('\n') {
                out.push_str(eol);
            }
            out.push_str(&yaml_render_entry(name, fields, ind, eol)?);
            lines[to..].iter().for_each(|l| out.push_str(l));
        }
    }
    Some(out)
}

fn yaml_splice_remove(text: &str, path: &[&str], name: &str) -> Option<String> {
    let [key] = path else { return None };
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let (top, true) = yaml_top_key(&lines, key)?? else {
        return None;
    };
    let end = yaml_region_end(&lines, top);
    let (_, Some((from, to))) = yaml_find_entry(&lines, top + 1..end, name)? else {
        return None;
    };
    let rest: Vec<&str> = lines[top + 1..from]
        .iter()
        .chain(&lines[to..end])
        .copied()
        .collect();
    let mut out = String::with_capacity(text.len());
    lines[..top].iter().for_each(|l| out.push_str(l));
    if rest.iter().any(|l| yaml_has_content(l)) {
        out.push_str(lines[top]);
    } else if rest.iter().any(|l| !yaml_line_body(l).trim().is_empty()) {
        // Only comments left under it: keep the key, as an empty mapping.
        let body = yaml_line_body(lines[top]);
        let (k, after) = yaml_plain_key(body)?;
        out.push_str(&format!("{k}: {{}}{after}"));
        out.push_str(&lines[top][body.len()..]);
    }
    // else: nothing else under it, so the key goes too (what an append adds).
    lines[top + 1..from].iter().for_each(|l| out.push_str(l));
    lines[to..].iter().for_each(|l| out.push_str(l));
    Some(out)
}

/// `root` without a top-level `path[0]` that is empty or null (a removal
/// may drop or keep an emptied parent).
fn without_empty_top(mut root: JsonMap, path: &[&str]) -> JsonMap {
    if let [key] = path
        && root
            .get(*key)
            .is_some_and(|v| v.is_null() || v.as_object().is_some_and(|o| o.is_empty()))
    {
        root.remove(*key);
    }
    root
}

// ------------------------------------------------------------------ JSON

fn parse_json(file: &Path, text: &str) -> Result<CstRootNode> {
    let text = if text.trim().is_empty() { "{}" } else { text };
    // Strip a UTF-8 BOM some editors write.
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    CstRootNode::parse(text, &jsonc_parser::ParseOptions::default())
        .map_err(|e| malformed(file, Format::Json, e))
}

fn json_container(
    file: &Path,
    root: &CstRootNode,
    path: &[&str],
    create: bool,
) -> Result<Option<CstObject>> {
    let mut obj = match root.value() {
        None if create => root.object_value_or_set(),
        None => return Ok(None),
        Some(_) => root.object_value().ok_or_else(|| Error::Malformed {
            path: file.to_path_buf(),
            detail: "the top level is not a JSON object".into(),
        })?,
    };
    for (i, seg) in path.iter().enumerate() {
        let next = match obj.get(seg) {
            Some(p) => match p.value() {
                Some(v) if v.as_object().is_some() => v.as_object(),
                // `"mcpServers": null` is treated as absent.
                Some(v) if v.as_null_keyword().is_some() && create => Some(p.object_value_or_set()),
                Some(v) if v.as_null_keyword().is_some() => None,
                _ => {
                    return Err(Error::Malformed {
                        path: file.to_path_buf(),
                        detail: format!("\"{}\" is not an object", path[..=i].join(".")),
                    });
                }
            },
            None if create => Some(obj.object_value_or_set(seg)),
            None => None,
        };
        match next {
            Some(n) => obj = n,
            None => return Ok(None),
        }
    }
    Ok(Some(obj))
}

fn fields_to_cst(fields: &Fields) -> CstInputValue {
    CstInputValue::Object(fields.iter().map(|(k, v)| (k.clone(), to_cst(v))).collect())
}

fn to_cst(v: &Value) -> CstInputValue {
    match v {
        Value::Null => CstInputValue::Null,
        Value::Bool(b) => CstInputValue::Bool(*b),
        Value::Number(n) => CstInputValue::Number(n.to_string()),
        Value::String(s) => CstInputValue::String(s.clone()),
        Value::Array(a) => CstInputValue::Array(a.iter().map(to_cst).collect()),
        Value::Object(o) => {
            CstInputValue::Object(o.iter().map(|(k, v)| (k.clone(), to_cst(v))).collect())
        }
    }
}

// ------------------------------------------------------------------ TOML

fn parse_toml(file: &Path, text: &str) -> Result<toml_edit::DocumentMut> {
    text.parse::<toml_edit::DocumentMut>()
        .map_err(|e| malformed(file, Format::Toml, e.to_string().trim()))
}

fn toml_container_ref<'a>(
    file: &Path,
    doc: &'a toml_edit::DocumentMut,
    path: &[&str],
) -> Result<Option<&'a dyn toml_edit::TableLike>> {
    let mut t: &dyn toml_edit::TableLike = doc.as_table();
    for (i, seg) in path.iter().enumerate() {
        match t.get(seg) {
            None => return Ok(None),
            Some(item) => {
                t = item.as_table_like().ok_or_else(|| Error::Malformed {
                    path: file.to_path_buf(),
                    detail: format!("\"{}\" is not a table", path[..=i].join(".")),
                })?;
            }
        }
    }
    Ok(Some(t))
}

fn toml_container_mut<'a>(
    file: &Path,
    doc: &'a mut toml_edit::DocumentMut,
    path: &[&str],
) -> Result<&'a mut dyn toml_edit::TableLike> {
    let mut t: &mut dyn toml_edit::TableLike = doc.as_table_mut();
    for (i, seg) in path.iter().enumerate() {
        let item = t.entry(seg).or_insert_with(|| {
            let mut n = toml_edit::Table::new();
            // `[mcp_servers.cua]` without an empty `[mcp_servers]` header.
            n.set_implicit(true);
            toml_edit::Item::Table(n)
        });
        t = item.as_table_like_mut().ok_or_else(|| Error::Malformed {
            path: file.to_path_buf(),
            detail: format!("\"{}\" is not a table", path[..=i].join(".")),
        })?;
    }
    Ok(t)
}

/// The entry becomes a `[table]`; nested objects become inline tables
/// (`env = { K = "v" }`, the style Codex documents).
fn fields_to_toml(fields: &Fields) -> toml_edit::Item {
    let mut t = toml_edit::Table::new();
    for (k, v) in fields {
        t.insert(k, toml_edit::Item::Value(json_to_toml_value(v)));
    }
    toml_edit::Item::Table(t)
}

fn json_to_toml_value(v: &Value) -> toml_edit::Value {
    match v {
        // TOML has no null; callers never pass one, keep it harmless.
        Value::Null => toml_edit::Value::from(""),
        Value::Bool(b) => toml_edit::Value::from(*b),
        Value::Number(n) => match n.as_i64() {
            Some(i) => toml_edit::Value::from(i),
            None => toml_edit::Value::from(n.as_f64().unwrap_or_default()),
        },
        Value::String(s) => toml_edit::Value::from(s.as_str()),
        Value::Array(a) => {
            let mut arr = toml_edit::Array::new();
            for x in a {
                arr.push(json_to_toml_value(x));
            }
            toml_edit::Value::Array(arr)
        }
        Value::Object(o) => {
            let mut t = toml_edit::InlineTable::new();
            for (k, v) in o {
                t.insert(k, json_to_toml_value(v));
            }
            toml_edit::Value::InlineTable(t)
        }
    }
}

fn toml_item_to_json(item: &toml_edit::Item) -> Value {
    match item {
        toml_edit::Item::None => Value::Null,
        toml_edit::Item::Value(v) => toml_value_to_json(v),
        toml_edit::Item::Table(t) => Value::Object(
            t.iter()
                .map(|(k, v)| (k.to_string(), toml_item_to_json(v)))
                .collect(),
        ),
        toml_edit::Item::ArrayOfTables(a) => Value::Array(
            a.iter()
                .map(|t| {
                    Value::Object(
                        t.iter()
                            .map(|(k, v)| (k.to_string(), toml_item_to_json(v)))
                            .collect(),
                    )
                })
                .collect(),
        ),
    }
}

fn toml_value_to_json(v: &toml_edit::Value) -> Value {
    use toml_edit::Value as V;
    match v {
        V::String(s) => Value::String(s.value().clone()),
        V::Integer(i) => Value::from(*i.value()),
        V::Float(f) => serde_json::Number::from_f64(*f.value())
            .map(Value::Number)
            .unwrap_or(Value::Null),
        V::Boolean(b) => Value::Bool(*b.value()),
        V::Datetime(d) => Value::String(d.value().to_string()),
        V::Array(a) => Value::Array(a.iter().map(toml_value_to_json).collect()),
        V::InlineTable(t) => Value::Object(
            t.iter()
                .map(|(k, v)| (k.to_string(), toml_value_to_json(v)))
                .collect(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn p() -> &'static Path {
        Path::new("/x/config")
    }

    /// Ordered fields from `(key, value)` pairs.
    fn f(pairs: &[(&str, Value)]) -> Fields {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect()
    }

    #[test]
    fn jsonc_upsert_keeps_comments_trailing_commas_and_other_servers() {
        let text = r#"{
  // user settings
  "theme": "dark",
  "mcpServers": {
    /* my server */
    "other": { "command": "other-bin", "args": ["--x"] },
  },
}
"#;
        let v = f(&[("command", json!("cua")), ("args", json!(["mcp"]))]);
        let out = upsert(Format::Json, p(), text, &["mcpServers"], "cua", &v).unwrap();
        assert!(out.contains("// user settings"), "{out}");
        assert!(out.contains("/* my server */"), "{out}");
        assert!(
            out.contains(r#""other": { "command": "other-bin", "args": ["--x"] }"#),
            "{out}"
        );
        assert_eq!(
            get(Format::Json, p(), &out, &["mcpServers"], "cua").unwrap(),
            Some(to_value(&v))
        );
        // Idempotent: a second upsert is a no-op.
        let again = upsert(Format::Json, p(), &out, &["mcpServers"], "cua", &v).unwrap();
        assert_eq!(again, out);
        let mut names = names(Format::Json, p(), &out, &["mcpServers"]).unwrap();
        names.sort();
        assert_eq!(names, ["cua", "other"]);
        // Remove restores the other entries and comments.
        let removed = remove(Format::Json, p(), &out, &["mcpServers"], "cua")
            .unwrap()
            .unwrap();
        assert!(removed.contains("/* my server */"));
        assert!(
            get(Format::Json, p(), &removed, &["mcpServers"], "cua")
                .unwrap()
                .is_none()
        );
        assert!(
            get(Format::Json, p(), &removed, &["mcpServers"], "other")
                .unwrap()
                .is_some()
        );
        assert!(
            remove(Format::Json, p(), &removed, &["mcpServers"], "cua")
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn json_creates_missing_file_and_nested_parents() {
        let v = f(&[("type", json!("local")), ("command", json!(["cua", "mcp"]))]);
        let out = upsert(Format::Json, p(), "", &["mcp"], "cua", &v).unwrap();
        let parsed: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(parsed["mcp"]["cua"], to_value(&v));
        let out = upsert(Format::Json, p(), "{}", &["a", "b"], "cua", &v).unwrap();
        let parsed: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(parsed["a"]["b"]["cua"], to_value(&v));
        // Dotted key names are literal (Amp's "amp.mcpServers").
        let out = upsert(
            Format::Json,
            p(),
            "{\"amp.x\": 1}",
            &["amp.mcpServers"],
            "cua",
            &v,
        )
        .unwrap();
        let parsed: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(parsed["amp.mcpServers"]["cua"], to_value(&v));
        assert_eq!(parsed["amp.x"], 1);
    }

    #[test]
    fn json_replaces_a_stale_entry_in_place() {
        let text = "{\"mcpServers\":{\"a\":{},\"cua\":{\"command\":\"old\"},\"z\":{}}}";
        let v = f(&[("command", json!("cua")), ("args", json!(["mcp"]))]);
        let out = upsert(Format::Json, p(), text, &["mcpServers"], "cua", &v).unwrap();
        let order = names(Format::Json, p(), &out, &["mcpServers"]).unwrap();
        assert_eq!(order, ["a", "cua", "z"]);
    }

    #[test]
    fn malformed_json_and_wrong_shapes_are_clear_errors() {
        let v: Fields = vec![];
        let e = upsert(
            Format::Json,
            p(),
            "{\"mcpServers\": {",
            &["mcpServers"],
            "cua",
            &v,
        )
        .unwrap_err();
        assert!(matches!(e, Error::Malformed { .. }), "{e}");
        assert!(e.to_string().contains("/x/config"), "{e}");
        let e = upsert(Format::Json, p(), "[1,2]", &["mcpServers"], "cua", &v).unwrap_err();
        assert!(e.to_string().contains("not a JSON object"), "{e}");
        let e = upsert(
            Format::Json,
            p(),
            "{\"mcpServers\": []}",
            &["mcpServers"],
            "cua",
            &v,
        )
        .unwrap_err();
        assert!(
            e.to_string().contains("\"mcpServers\" is not an object"),
            "{e}"
        );
        // null is treated as absent.
        let out = upsert(
            Format::Json,
            p(),
            "{\"mcpServers\": null}",
            &["mcpServers"],
            "cua",
            &v,
        )
        .unwrap();
        assert!(
            get(Format::Json, p(), &out, &["mcpServers"], "cua")
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn json5ish_loose_keys_parse() {
        let text = "{\n  // json5\n  mcp: { servers: { other: { command: 'x' } } },\n}\n";
        let v = f(&[("command", json!("cua"))]);
        let out = upsert(Format::Json, p(), text, &["mcp", "servers"], "cua", &v).unwrap();
        assert!(out.contains("// json5"));
        assert!(out.contains("other: { command: 'x' }"), "{out}");
    }

    #[test]
    fn toml_upsert_keeps_comments_and_other_tables() {
        let text = r#"# Codex config
model = "gpt-5" # inline comment

[mcp_servers.other]
command = "other"
args = ["a"] # keep me

[profiles.fast]
model = "x"
"#;
        let v = f(&[
            ("command", json!("/usr/local/bin/cua")),
            ("args", json!(["mcp"])),
            ("env", json!({"CUA_X": "1"})),
        ]);
        let out = upsert(Format::Toml, p(), text, &["mcp_servers"], "cua", &v).unwrap();
        assert!(out.contains("# Codex config"));
        assert!(out.contains("model = \"gpt-5\" # inline comment"));
        assert!(out.contains("args = [\"a\"] # keep me"));
        assert!(out.contains("[mcp_servers.cua]"), "{out}");
        assert!(!out.contains("[mcp_servers]\n"), "{out}");
        assert!(out.contains("env = { CUA_X = \"1\" }"), "{out}");
        assert_eq!(
            get(Format::Toml, p(), &out, &["mcp_servers"], "cua").unwrap(),
            Some(to_value(&v))
        );
        assert_eq!(
            upsert(Format::Toml, p(), &out, &["mcp_servers"], "cua", &v).unwrap(),
            out
        );
        let removed = remove(Format::Toml, p(), &out, &["mcp_servers"], "cua")
            .unwrap()
            .unwrap();
        assert_eq!(removed, text);
    }

    #[test]
    fn yaml_edits_keep_comments_and_other_entries() {
        let text = "GOOSE_PROVIDER: anthropic\nextensions:\n  developer:\n    enabled: true\n    type: builtin\n";
        let v = f(&[
            ("type", json!("stdio")),
            ("name", json!("cua")),
            ("cmd", json!("cua")),
            ("args", json!(["mcp"])),
        ]);
        for prefix in ["", "# mine\n"] {
            let text = format!("{prefix}{text}");
            let out = upsert(Format::Yaml, p(), &text, &["extensions"], "cua", &v).unwrap();
            assert!(out.starts_with(&text), "appended after the others:\n{out}");
            assert!(
                out.contains(
                    "  cua:\n    type: stdio\n    name: cua\n    cmd: cua\n    args:\n      - mcp\n"
                ),
                "{out}"
            );
            assert_eq!(
                get(Format::Yaml, p(), &out, &["extensions"], "cua").unwrap(),
                Some(to_value(&v))
            );
            assert_eq!(
                upsert(Format::Yaml, p(), &out, &["extensions"], "cua", &v).unwrap(),
                out,
                "idempotent"
            );
            let removed = remove(Format::Yaml, p(), &out, &["extensions"], "cua")
                .unwrap()
                .unwrap();
            assert_eq!(removed, text, "removal restores the file byte for byte");
        }
        let e = upsert(
            Format::Yaml,
            p(),
            "extensions: [\n",
            &["extensions"],
            "cua",
            &v,
        )
        .unwrap_err();
        assert!(matches!(e, Error::Malformed { .. }));
        let new = upsert(Format::Yaml, p(), "", &["extensions"], "cua", &v).unwrap();
        assert!(new.starts_with("extensions:\n  cua:"), "{new}");
    }

    #[test]
    fn yaml_layouts_the_splice_does_not_handle_fall_back_or_refuse() {
        let v = f(&[("command", json!("cua")), ("args", json!(["mcp"]))]);
        // A flow mapping with content: comment-free files go through serde.
        let flow = "mcp_servers: {other: {command: o}}\n";
        let out = upsert(Format::Yaml, p(), flow, &["mcp_servers"], "cua", &v).unwrap();
        assert!(
            get(Format::Yaml, p(), &out, &["mcp_servers"], "other")
                .unwrap()
                .is_some()
        );
        // ... and commented ones are refused rather than losing comments.
        let commented = format!("# mine\n{flow}");
        let e = upsert(Format::Yaml, p(), &commented, &["mcp_servers"], "cua", &v).unwrap_err();
        assert!(e.to_string().contains("comments"), "{e}");
        // Nested key paths are never spliced.
        let nested = "# c\na:\n  b:\n    x: {}\n";
        let e = upsert(Format::Yaml, p(), nested, &["a", "b"], "cua", &v).unwrap_err();
        assert!(e.to_string().contains("comments"), "{e}");
        // A sequence where a mapping is expected is a clear error.
        let e = upsert(
            Format::Yaml,
            p(),
            "mcp_servers:\n- a\n",
            &["mcp_servers"],
            "cua",
            &v,
        )
        .unwrap_err();
        assert!(e.to_string().contains("not a mapping"), "{e}");
    }

    #[test]
    fn yaml_splice_handles_hermes_style_configs() {
        let v = f(&[
            ("command", json!("/opt/cua/bin/cua")),
            ("args", json!(["mcp"])),
            ("env", json!({"CUA_X": "1"})),
        ]);
        let key: &[&str] = &["mcp_servers"];
        // The seeded template: a commented-out example, no live key.
        let template =
            "model:\n  default: x  # pick one\n\n# mcp_servers:\n#   time:\n#     command: uvx\n";
        let out = upsert(Format::Yaml, p(), template, key, "cua", &v).unwrap();
        assert_eq!(
            out,
            format!(
                "{template}mcp_servers:\n  cua:\n    command: /opt/cua/bin/cua\n    args:\n      - mcp\n    env:\n      CUA_X: '1'\n"
            )
        );
        assert_eq!(
            remove(Format::Yaml, p(), &out, key, "cua")
                .unwrap()
                .unwrap(),
            template
        );

        // Live servers with comments: a stale cua entry in the middle is
        // replaced in place; its neighbours, their comments and the
        // section after it stay.
        let live = "mcp_servers:   # my servers\n    time:\n        command: uvx  # keep\n        args: [\"mcp-server-time\"]\n\n    cua:\n        command: old\n    # between\n    github:\n        url: https://x\n# trailing note\nagent:\n  max_turns: 5\n";
        let out = upsert(Format::Yaml, p(), live, key, "cua", &v).unwrap();
        assert_eq!(
            out,
            "mcp_servers:   # my servers\n    time:\n        command: uvx  # keep\n        args: [\"mcp-server-time\"]\n\n    cua:\n      command: /opt/cua/bin/cua\n      args:\n        - mcp\n      env:\n        CUA_X: '1'\n    # between\n    github:\n        url: https://x\n# trailing note\nagent:\n  max_turns: 5\n"
        );
        let removed = remove(Format::Yaml, p(), &out, key, "cua")
            .unwrap()
            .unwrap();
        assert_eq!(
            removed,
            "mcp_servers:   # my servers\n    time:\n        command: uvx  # keep\n        args: [\"mcp-server-time\"]\n\n    # between\n    github:\n        url: https://x\n# trailing note\nagent:\n  max_turns: 5\n"
        );

        // Inline empty values and CRLF files.
        for (text, want_head) in [
            (
                "a: 1\nmcp_servers: {}\nb: 2\n",
                "a: 1\nmcp_servers:\n  cua:\n",
            ),
            (
                "# c\r\nmcp_servers: null\r\n",
                "# c\r\nmcp_servers:\r\n  cua:\r\n",
            ),
            (
                "# c\nmcp_servers:\n# nothing yet\nz: 1",
                "# c\nmcp_servers:\n  cua:\n",
            ),
        ] {
            let out = upsert(Format::Yaml, p(), text, key, "cua", &v).unwrap();
            assert!(out.starts_with(want_head), "{out:?}");
            assert_eq!(
                get(Format::Yaml, p(), &out, key, "cua").unwrap(),
                Some(to_value(&v)),
                "{out:?}"
            );
            assert!(
                !out.contains("\r\n") || !out.replace("\r\n", "").contains('\n'),
                "{out:?}"
            );
        }

        // Removing the only entry under a key that also holds comments keeps
        // the key as an empty mapping (so the comments stay put).
        let lone = "mcp_servers:  # mine\n  # disabled: foo\n  cua:\n    command: x\n";
        let removed = remove(Format::Yaml, p(), lone, key, "cua")
            .unwrap()
            .unwrap();
        assert_eq!(removed, "mcp_servers: {}  # mine\n  # disabled: foo\n");
        assert!(
            remove(Format::Yaml, p(), &removed, key, "cua")
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn toml_new_file_and_errors() {
        let v = f(&[("command", json!("cua")), ("args", json!(["mcp"]))]);
        let out = upsert(Format::Toml, p(), "", &["mcp_servers"], "cua", &v).unwrap();
        assert_eq!(
            out,
            "[mcp_servers.cua]\ncommand = \"cua\"\nargs = [\"mcp\"]\n"
        );
        let e = upsert(
            Format::Toml,
            p(),
            "[mcp_servers\n",
            &["mcp_servers"],
            "cua",
            &v,
        )
        .unwrap_err();
        assert!(matches!(e, Error::Malformed { .. }));
        let e = upsert(
            Format::Toml,
            p(),
            "mcp_servers = 3\n",
            &["mcp_servers"],
            "cua",
            &v,
        )
        .unwrap_err();
        assert!(e.to_string().contains("not a table"), "{e}");
        // Inline-table style written by hand is edited in place.
        let out = upsert(
            Format::Toml,
            p(),
            "mcp_servers = { other = { command = \"o\" } }\n",
            &["mcp_servers"],
            "cua",
            &v,
        )
        .unwrap();
        assert!(
            get(Format::Toml, p(), &out, &["mcp_servers"], "other")
                .unwrap()
                .is_some()
        );
        assert_eq!(
            get(Format::Toml, p(), &out, &["mcp_servers"], "cua").unwrap(),
            Some(to_value(&v))
        );
    }
}
