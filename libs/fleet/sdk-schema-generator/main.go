package main

import (
	"bytes"
	"flag"
	"fmt"
	goformat "go/format"
	"io"
	"os"
	"os/exec"
	"path/filepath"
	"sort"
	"strings"
	"unicode"

	yamlv3 "gopkg.in/yaml.v3"
)

type node struct {
	Name       string
	JSONName   string
	Type       string
	Required   bool
	Enum       []string
	Properties []*node
	Items      *node
	MapValues  *node
	Opaque     bool
}

type rootType struct {
	CRDName string
	Version string
	Name    string
	Node    *node
}

type output struct {
	Path string
	Data []byte
}

func main() {
	root := flag.String("root", ".", "repository root")
	check := flag.Bool("check", false, "fail if generated files differ")
	flag.Parse()

	crdPath := filepath.Join(*root, "clusters/base/osgym/crd.yaml")
	roots := []*rootType{
		{CRDName: "osgymworkspacepools.cua.ai", Version: "v1", Name: "PoolSpec"},
		{CRDName: "osgymsandboxclaims.osgym.cua.ai", Version: "v1alpha1", Name: "ClaimSpec"},
	}
	loadRoots(crdPath, roots)

	outputs := []output{
		{filepath.Join(*root, "cyclops-cs/sdk-core/wit/generated-crd.wit"), renderWIT(roots)},
		{filepath.Join(*root, "cyclops-cs/sdk-core/src/generated_crd.rs"), formatRust(renderRust(roots))},
		{filepath.Join(*root, "cyclops-cs/sdk-core/src/generated_crd_component.rs"), formatRust(renderComponent(roots))},
		{filepath.Join(*root, "cyclops-cs/sdk-bindings/typescript/src/generated-crd.ts"), renderTypeScript(roots)},
		{filepath.Join(*root, "cyclops-cs/sdk-bindings/python/cyclops_sdk/generated_crd.py"), renderPython(roots)},
		{filepath.Join(*root, "cyclops-cs/sdk-bindings/go/generated_crd.go"), formatGo(renderGo(roots))},
	}

	failed := false
	for _, item := range outputs {
		current, err := os.ReadFile(item.Path)
		if *check {
			if err != nil || !bytes.Equal(current, item.Data) {
				fmt.Fprintf(os.Stderr, "generated file is stale: %s\n", item.Path)
				failed = true
			}
			continue
		}
		if err := os.MkdirAll(filepath.Dir(item.Path), 0o755); err != nil {
			fatalf("create output directory: %v", err)
		}
		if err := os.WriteFile(item.Path, item.Data, 0o644); err != nil {
			fatalf("write %s: %v", item.Path, err)
		}
	}
	if failed {
		os.Exit(1)
	}
}

func formatRust(source []byte) []byte {
	command := exec.Command("rustfmt", "--emit", "stdout")
	command.Stdin = bytes.NewReader(source)
	formatted, err := command.Output()
	if err != nil {
		fatalf("rustfmt generated source: %v", err)
	}
	return formatted
}

func formatGo(source []byte) []byte {
	formatted, err := goformat.Source(source)
	if err != nil {
		fatalf("gofmt generated source: %v", err)
	}
	return formatted
}

func loadRoots(path string, roots []*rootType) {
	data, err := os.ReadFile(path)
	if err != nil {
		fatalf("read CRDs: %v", err)
	}
	wanted := map[string]*rootType{}
	for _, root := range roots {
		wanted[root.CRDName] = root
	}
	decoder := yamlv3.NewDecoder(bytes.NewReader(data))
	for {
		var document map[string]any
		if err := decoder.Decode(&document); err != nil {
			if err == io.EOF {
				break
			}
			fatalf("decode CRDs: %v", err)
		}
		metadata, _ := document["metadata"].(map[string]any)
		root := wanted[fmt.Sprint(metadata["name"])]
		if root == nil {
			continue
		}
		versions := mustSlice(mustMap(document["spec"], "spec")["versions"], "spec.versions")
		for _, raw := range versions {
			version := mustMap(raw, "version")
			if fmt.Sprint(version["name"]) != root.Version {
				continue
			}
			schema := mustMap(mustMap(version["schema"], "schema")["openAPIV3Schema"], "openAPIV3Schema")
			spec := property(schema, "spec")
			root.Node = parseNode(root.Name, "spec", spec, true)
		}
	}
	for _, root := range roots {
		if root.Node == nil {
			fatalf("missing %s %s schema", root.CRDName, root.Version)
		}
	}
}

func parseNode(name, jsonName string, schema map[string]any, required bool) *node {
	result := &node{Name: name, JSONName: jsonName, Type: fmt.Sprint(schema["type"]), Required: required}
	if values, ok := schema["enum"].([]any); ok {
		for _, value := range values {
			result.Enum = append(result.Enum, fmt.Sprint(value))
		}
	}
	if schema["x-kubernetes-preserve-unknown-fields"] == true {
		result.Opaque = true
		return result
	}
	switch result.Type {
	case "object":
		if additional, ok := schema["additionalProperties"].(map[string]any); ok {
			result.MapValues = parseNode(name+"Value", "value", additional, true)
			return result
		}
		properties, _ := schema["properties"].(map[string]any)
		requiredSet := stringSet(schema["required"])
		keys := sortedKeys(properties)
		for _, key := range keys {
			childName := name + exportedName(key)
			if key == "template" && strings.HasSuffix(name, "Spec") {
				childName = strings.TrimSuffix(name, "Spec") + "Template"
			}
			if key == "sandboxTemplateRef" {
				childName = "SandboxTemplateRef"
			}
			result.Properties = append(result.Properties, parseNode(childName, key, mustMap(properties[key], key), requiredSet[key]))
		}
	case "array":
		result.Items = parseNode(strings.TrimSuffix(name, "s"), "item", mustMap(schema["items"], name+".items"), true)
	}
	return result
}

func renderWIT(roots []*rootType) []byte {
	var out bytes.Buffer
	out.WriteString("// Code generated from clusters/base/osgym/crd.yaml; DO NOT EDIT.\n")
	out.WriteString("package trycua:cyclops-sdk@1.0.0;\n\ninterface crd-types {\n")
	nodes := collectNamed(roots)
	if hasMap(nodes) {
		out.WriteString("  record string-map-entry { key: string, value: string }\n")
	}
	for _, item := range nodes {
		if len(item.Enum) > 0 {
			fmt.Fprintf(&out, "  enum %s { %s }\n", witName(item.Name), strings.Join(enumWIT(item.Enum), ", "))
			continue
		}
		if item.Type != "object" || item.MapValues != nil || item.Opaque {
			continue
		}
		fmt.Fprintf(&out, "  record %s {\n", witName(item.Name))
		for _, field := range item.Properties {
			fieldType := witType(field)
			if !field.Required {
				fieldType = "option<" + fieldType + ">"
			}
			fmt.Fprintf(&out, "    %s: %s,\n", witName(field.JSONName), fieldType)
		}
		out.WriteString("  }\n")
	}
	out.WriteString("}\n")
	return out.Bytes()
}

func renderRust(roots []*rootType) []byte {
	var out bytes.Buffer
	out.WriteString("// Code generated from clusters/base/osgym/crd.yaml; DO NOT EDIT.\n")
	out.WriteString("use serde::{Deserialize, Serialize};\n\n")
	for _, item := range collectNamed(roots) {
		if len(item.Enum) > 0 {
			out.WriteString("#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]\n")
			fmt.Fprintf(&out, "pub enum %s {\n", item.Name)
			for _, value := range item.Enum {
				fmt.Fprintf(&out, "    #[serde(rename = %q)]\n    %s,\n", value, exportedName(value))
			}
			out.WriteString("}\n\n")
			continue
		}
		if item.Type != "object" || item.MapValues != nil || item.Opaque {
			continue
		}
		out.WriteString("#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]\n")
		fmt.Fprintf(&out, "pub struct %s {\n", item.Name)
		for _, field := range item.Properties {
			fieldType := rustType(field)
			if !field.Required {
				fieldType = "Option<" + fieldType + ">"
				out.WriteString("    #[serde(default, skip_serializing_if = \"Option::is_none\")]\n")
			}
			fmt.Fprintf(&out, "    #[serde(rename = %q)]\n    pub %s: %s,\n", field.JSONName, rustField(field.JSONName), fieldType)
		}
		out.WriteString("}\n\n")
		renderRustBuilder(&out, item)
	}
	return out.Bytes()
}

func renderRustBuilder(out *bytes.Buffer, item *node) {
	fmt.Fprintf(out, "impl %s {\n", item.Name)
	fmt.Fprintf(out, "    pub fn builder() -> %sBuilder { %sBuilder::default() }\n", item.Name, item.Name)
	out.WriteString("}\n\n")
	fmt.Fprintf(out, "#[derive(Debug, Clone, Default)]\npub struct %sBuilder {\n", item.Name)
	for _, field := range item.Properties {
		fmt.Fprintf(out, "    %s: Option<%s>,\n", rustField(field.JSONName), rustType(field))
	}
	out.WriteString("}\n\n")
	fmt.Fprintf(out, "impl %sBuilder {\n", item.Name)
	for _, field := range item.Properties {
		fieldName := rustField(field.JSONName)
		fieldType := rustType(field)
		fmt.Fprintf(out, "    pub fn %s(mut self, value: %s) -> Self {\n", fieldName, fieldType)
		fmt.Fprintf(out, "        self.%s = Some(value);\n        self\n    }\n", fieldName)
	}
	fmt.Fprintf(out, "    pub fn build(self) -> Result<%s, String> {\n", item.Name)
	fmt.Fprintf(out, "        Ok(%s {\n", item.Name)
	for _, field := range item.Properties {
		fieldName := rustField(field.JSONName)
		if field.Required {
			fmt.Fprintf(out, "            %s: self.%s.ok_or_else(|| %q.to_string())?,\n", fieldName, fieldName, field.JSONName+" is required")
		} else {
			fmt.Fprintf(out, "            %s: self.%s,\n", fieldName, fieldName)
		}
	}
	out.WriteString("        })\n    }\n}\n\n")
}

func renderComponent(roots []*rootType) []byte {
	var out bytes.Buffer
	out.WriteString("// Code generated from clusters/base/osgym/crd.yaml; DO NOT EDIT.\n")
	for _, item := range collectNamed(roots) {
		if len(item.Enum) > 0 {
			fmt.Fprintf(&out, "fn %s_json(value: crd_types::%s) -> serde_json::Value {\n", snake(item.Name), item.Name)
			out.WriteString("    serde_json::Value::String(match value {\n")
			for _, value := range item.Enum {
				fmt.Fprintf(&out, "        crd_types::%s::%s => %q,\n", item.Name, exportedName(strings.ToLower(value)), value)
			}
			out.WriteString("    }.into())\n}\n\n")
			continue
		}
		if item.Type != "object" || item.MapValues != nil || item.Opaque {
			continue
		}
		fmt.Fprintf(&out, "fn %s_json(value: crd_types::%s) -> Result<serde_json::Value, String> {\n", snake(item.Name), item.Name)
		out.WriteString("    let mut object = serde_json::Map::new();\n")
		for _, field := range item.Properties {
			fieldAccess := "value." + rustField(field.JSONName)
			if field.Required {
				fmt.Fprintf(&out, "    object.insert(%q.into(), %s);\n", field.JSONName, componentValue(field, fieldAccess))
			} else {
				fmt.Fprintf(&out, "    if let Some(value) = %s { object.insert(%q.into(), %s); }\n", fieldAccess, field.JSONName, componentValue(field, "value"))
			}
		}
		out.WriteString("    Ok(serde_json::Value::Object(object))\n}\n\n")
	}
	for _, root := range roots {
		fmt.Fprintf(&out, "fn %s_from_wit(value: crd_types::%s) -> Result<super::%s, String> {\n", snake(root.Name), root.Name, root.Name)
		fmt.Fprintf(&out, "    serde_json::from_value(%s_json(value)?).map_err(|error| error.to_string())\n}\n\n", snake(root.Name))
	}
	return out.Bytes()
}

func renderTypeScript(roots []*rootType) []byte {
	var out bytes.Buffer
	out.WriteString("// Code generated from clusters/base/osgym/crd.yaml; DO NOT EDIT.\n")
	for _, item := range collectNamed(roots) {
		if len(item.Enum) > 0 {
			fmt.Fprintf(&out, "export type %s = %s\n", item.Name, quoteUnion(item.Enum))
			continue
		}
		if item.Type != "object" || item.MapValues != nil || item.Opaque {
			continue
		}
		fmt.Fprintf(&out, "export type %s = {\n", item.Name)
		for _, field := range item.Properties {
			optional := ""
			if !field.Required {
				optional = "?"
			}
			fmt.Fprintf(&out, "  %s%s: %s\n", field.JSONName, optional, tsType(field))
		}
		out.WriteString("}\n")
	}
	return out.Bytes()
}

func renderPython(roots []*rootType) []byte {
	var out bytes.Buffer
	out.WriteString("# Code generated from clusters/base/osgym/crd.yaml; DO NOT EDIT.\n")
	out.WriteString("from __future__ import annotations\n\n")
	out.WriteString("from typing import Any, Literal, NotRequired, TypedDict\n\n")
	for _, item := range collectNamed(roots) {
		if len(item.Enum) > 0 {
			fmt.Fprintf(&out, "%s = Literal[%s]\n\n", item.Name, quotedCSV(item.Enum))
			continue
		}
		if item.Type != "object" || item.MapValues != nil || item.Opaque {
			continue
		}
		fmt.Fprintf(&out, "class %s(TypedDict):\n", item.Name)
		if len(item.Properties) == 0 {
			out.WriteString("    pass\n")
		}
		for _, field := range item.Properties {
			fieldType := pyType(field)
			if !field.Required {
				fieldType = "NotRequired[" + fieldType + "]"
			}
			fmt.Fprintf(&out, "    %s: %s\n", field.JSONName, fieldType)
		}
		out.WriteString("\n")
	}
	return out.Bytes()
}

func renderGo(roots []*rootType) []byte {
	var out bytes.Buffer
	out.WriteString("// Code generated from clusters/base/osgym/crd.yaml; DO NOT EDIT.\n")
	out.WriteString("package cyclopssdk\n\n")
	for _, item := range collectNamed(roots) {
		if len(item.Enum) > 0 {
			fmt.Fprintf(&out, "type %s string\n\nconst (\n", item.Name)
			for _, value := range item.Enum {
				fmt.Fprintf(&out, "\t%s%s %s = %q\n", item.Name, exportedName(value), item.Name, value)
			}
			out.WriteString(")\n\n")
			continue
		}
		if item.Type != "object" || item.MapValues != nil || item.Opaque {
			continue
		}
		fmt.Fprintf(&out, "type %s struct {\n", item.Name)
		for _, field := range item.Properties {
			fieldType := goType(field)
			tag := field.JSONName
			if !field.Required {
				if goNeedsPointer(field) {
					fieldType = "*" + fieldType
				}
				tag += ",omitempty"
			}
			fmt.Fprintf(&out, "\t%s %s `json:%q`\n", exportedName(field.JSONName), fieldType, tag)
		}
		out.WriteString("}\n\n")
	}
	return out.Bytes()
}

func collectNamed(roots []*rootType) []*node {
	seen := map[string]*node{}
	var visit func(*node)
	visit = func(item *node) {
		if item == nil {
			return
		}
		if (item.Type == "object" && item.MapValues == nil && !item.Opaque) || len(item.Enum) > 0 {
			seen[item.Name] = item
		}
		for _, child := range item.Properties {
			visit(child)
		}
		visit(item.Items)
		visit(item.MapValues)
	}
	for _, root := range roots {
		visit(root.Node)
	}
	keys := make([]string, 0, len(seen))
	for key := range seen {
		keys = append(keys, key)
	}
	sort.Strings(keys)
	result := make([]*node, 0, len(keys))
	for _, key := range keys {
		result = append(result, seen[key])
	}
	return result
}

func witType(item *node) string {
	if len(item.Enum) > 0 || (item.Type == "object" && item.MapValues == nil && !item.Opaque) {
		return witName(item.Name)
	}
	if item.Opaque {
		return "string"
	}
	switch item.Type {
	case "string":
		return "string"
	case "integer":
		return "u32"
	case "boolean":
		return "bool"
	case "array":
		return "list<" + witType(item.Items) + ">"
	case "object":
		return "list<string-map-entry>"
	default:
		fatalf("unsupported WIT schema type %q for %s", item.Type, item.Name)
		return ""
	}
}

func rustType(item *node) string {
	if len(item.Enum) > 0 || (item.Type == "object" && item.MapValues == nil && !item.Opaque) {
		return item.Name
	}
	if item.Opaque {
		return "serde_json::Value"
	}
	switch item.Type {
	case "string":
		return "String"
	case "integer":
		return "u32"
	case "boolean":
		return "bool"
	case "array":
		return "Vec<" + rustType(item.Items) + ">"
	case "object":
		return "std::collections::BTreeMap<String, " + rustType(item.MapValues) + ">"
	default:
		fatalf("unsupported Rust schema type %q for %s", item.Type, item.Name)
		return ""
	}
}

func tsType(item *node) string {
	if len(item.Enum) > 0 || (item.Type == "object" && item.MapValues == nil && !item.Opaque) {
		return item.Name
	}
	if item.Opaque {
		return "unknown"
	}
	switch item.Type {
	case "string":
		return "string"
	case "integer":
		return "number"
	case "boolean":
		return "boolean"
	case "array":
		return tsType(item.Items) + "[]"
	case "object":
		return "Record<string, " + tsType(item.MapValues) + ">"
	default:
		fatalf("unsupported TypeScript schema type %q", item.Type)
		return "never"
	}
}

func pyType(item *node) string {
	if len(item.Enum) > 0 || (item.Type == "object" && item.MapValues == nil && !item.Opaque) {
		return item.Name
	}
	if item.Opaque {
		return "Any"
	}
	switch item.Type {
	case "string":
		return "str"
	case "integer":
		return "int"
	case "boolean":
		return "bool"
	case "array":
		return "list[" + pyType(item.Items) + "]"
	case "object":
		return "dict[str, " + pyType(item.MapValues) + "]"
	default:
		fatalf("unsupported Python schema type %q", item.Type)
		return "Any"
	}
}

func goType(item *node) string {
	if len(item.Enum) > 0 || (item.Type == "object" && item.MapValues == nil && !item.Opaque) {
		return item.Name
	}
	if item.Opaque {
		return "any"
	}
	switch item.Type {
	case "string":
		return "string"
	case "integer":
		return "uint32"
	case "boolean":
		return "bool"
	case "array":
		return "[]" + goType(item.Items)
	case "object":
		return "map[string]" + goType(item.MapValues)
	default:
		fatalf("unsupported Go schema type %q", item.Type)
		return "any"
	}
}

func componentValue(item *node, expression string) string {
	if len(item.Enum) > 0 {
		return snake(item.Name) + "_json(" + expression + ")"
	}
	if item.Opaque {
		return "serde_json::from_str(&" + expression + ").map_err(|error| error.to_string())?"
	}
	if item.Type == "object" && item.MapValues == nil {
		return snake(item.Name) + "_json(" + expression + ")?"
	}
	switch item.Type {
	case "string", "integer", "boolean":
		return "serde_json::Value::from(" + expression + ")"
	case "array":
		inner := componentValue(item.Items, "item")
		if strings.Contains(inner, "?") {
			return "serde_json::Value::Array(" + expression + ".into_iter().map(|item| Ok(" + inner + ")).collect::<Result<Vec<_>, String>>()?)"
		}
		return "serde_json::Value::Array(" + expression + ".into_iter().map(|item| " + inner + ").collect())"
	case "object":
		return "serde_json::Value::Object(" + expression + ".into_iter().map(|entry| (entry.key, serde_json::Value::from(entry.value))).collect())"
	default:
		fatalf("unsupported component type %q", item.Type)
		return ""
	}
}

func property(schema map[string]any, name string) map[string]any {
	return mustMap(mustMap(schema["properties"], "properties")[name], name)
}
func stringSet(value any) map[string]bool {
	result := map[string]bool{}
	for _, item := range mustSliceDefault(value) {
		result[fmt.Sprint(item)] = true
	}
	return result
}
func mustSliceDefault(value any) []any {
	if value == nil {
		return nil
	}
	return mustSlice(value, "array")
}
func sortedKeys(values map[string]any) []string {
	keys := make([]string, 0, len(values))
	for key := range values {
		keys = append(keys, key)
	}
	sort.Strings(keys)
	return keys
}
func hasMap(nodes []*node) bool {
	for _, item := range nodes {
		for _, field := range item.Properties {
			if field.Type == "object" && field.MapValues != nil {
				return true
			}
		}
	}
	return false
}
func goNeedsPointer(item *node) bool {
	return item.Type == "string" || item.Type == "integer" || item.Type == "boolean" || len(item.Enum) > 0 || (item.Type == "object" && item.MapValues == nil && !item.Opaque)
}
func exportedName(value string) string {
	parts := splitWords(value)
	var out strings.Builder
	for _, part := range parts {
		if part == "" {
			continue
		}
		runes := []rune(part)
		runes[0] = unicode.ToUpper(runes[0])
		out.WriteString(string(runes))
	}
	return out.String()
}
func splitWords(value string) []string {
	value = strings.ReplaceAll(value, "-", "_")
	var out []string
	start := 0
	runes := []rune(value)
	for i := 1; i < len(runes); i++ {
		if unicode.IsUpper(runes[i]) && (unicode.IsLower(runes[i-1]) || unicode.IsDigit(runes[i-1])) {
			out = append(out, string(runes[start:i]))
			start = i
		}
	}
	out = append(out, string(runes[start:]))
	return strings.FieldsFunc(strings.Join(out, "_"), func(r rune) bool { return r == '_' })
}
func snake(value string) string {
	parts := splitWords(value)
	for i := range parts {
		parts[i] = strings.ToLower(parts[i])
	}
	return strings.Join(parts, "_")
}
func witName(value string) string { return strings.ReplaceAll(snake(value), "_", "-") }
func rustField(value string) string {
	result := snake(value)
	if result == "type" {
		return "type_"
	}
	return result
}
func enumWIT(values []string) []string {
	result := make([]string, len(values))
	for i, value := range values {
		result[i] = witName(value)
	}
	return result
}
func quoteUnion(values []string) string {
	quoted := make([]string, len(values))
	for i, value := range values {
		quoted[i] = fmt.Sprintf("%q", value)
	}
	return strings.Join(quoted, " | ")
}
func quotedCSV(values []string) string {
	quoted := make([]string, len(values))
	for i, value := range values {
		quoted[i] = fmt.Sprintf("%q", value)
	}
	return strings.Join(quoted, ", ")
}
func mustMap(value any, name string) map[string]any {
	result, ok := value.(map[string]any)
	if !ok {
		fatalf("%s is not an object", name)
	}
	return result
}
func mustSlice(value any, name string) []any {
	result, ok := value.([]any)
	if !ok {
		fatalf("%s is not an array", name)
	}
	return result
}
func fatalf(format string, args ...any) { fmt.Fprintf(os.Stderr, format+"\n", args...); os.Exit(1) }
