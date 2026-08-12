package main

import (
	"bytes"
	"encoding/json"
	"flag"
	"fmt"
	"go/format"
	"io"
	"os"
	"sort"
	"strconv"
	"strings"

	yamlv3 "gopkg.in/yaml.v3"
)

type field struct {
	Name            string `json:"name"`
	GoName          string `json:"go_name"`
	ValueType       string `json:"value_type"`
	Mode            string `json:"mode"`
	CR              string `json:"cr"`
	CRDPath         string `json:"crd_path"`
	Description     string `json:"description"`
	DNSLabel        bool   `json:"dns_label"`
	RequiresReplace bool   `json:"requires_replace"`
	Minimum         *int64 `json:"minimum"`
	Maximum         *int64 `json:"maximum"`
}

type block struct {
	Name       string  `json:"name"`
	GoName     string  `json:"go_name"`
	Model      string  `json:"model"`
	Collection string  `json:"collection"`
	CR         string  `json:"cr"`
	CRDPath    string  `json:"crd_path"`
	Fields     []field `json:"fields"`
}

type mapping struct {
	ResourceDescription string  `json:"resource_description"`
	Attributes          []field `json:"attributes"`
	Blocks              []block `json:"blocks"`
}

type schemaInfo struct {
	Type        string
	Description string
	Enum        []string
	Minimum     *int64
	Maximum     *int64
}

// The flat fleets_pool resource is backed by a pair of native CRs, so every
// mapped attribute names the one it lives in.
const (
	warmPoolCR       = "warmpool"
	templateCR       = "template"
	warmPoolCRDName  = "osgymsandboxwarmpools.osgym.cua.ai"
	templateCRDName  = "osgymsandboxtemplates.osgym.cua.ai"
	nativeCRDVersion = "v1alpha1"
)

// crdSchemas holds the openAPIV3Schema of each CR keyed by mapping CR name.
type crdSchemas map[string]map[string]any

func main() {
	crdPath := flag.String("crd", "", "path to the osgym.cua.ai CRD bundle")
	mappingPath := flag.String("mapping", "", "path to the Terraform mapping JSON")
	outputPath := flag.String("out", "", "generated Go output path")
	flag.Parse()
	if *crdPath == "" || *mappingPath == "" || *outputPath == "" {
		fatalf("-crd, -mapping, and -out are required")
	}

	var config mapping
	readJSON(*mappingPath, &config)
	schemas := readCRDSchemas(*crdPath)
	validateMapping(schemas, config)

	generated, err := format.Source(render(schemas, config))
	if err != nil {
		fatalf("format generated source: %v\n%s", err, render(schemas, config))
	}
	if err := os.WriteFile(*outputPath, generated, 0o644); err != nil {
		fatalf("write generated source: %v", err)
	}
}

func readJSON(path string, target any) {
	data, err := os.ReadFile(path)
	if err != nil {
		fatalf("read %s: %v", path, err)
	}
	if err := json.Unmarshal(data, target); err != nil {
		fatalf("decode %s: %v", path, err)
	}
}

func readCRDSchemas(path string) crdSchemas {
	data, err := os.ReadFile(path)
	if err != nil {
		fatalf("read CRD bundle: %v", err)
	}
	wanted := map[string]string{warmPoolCRDName: warmPoolCR, templateCRDName: templateCR}
	schemas := crdSchemas{}
	decoder := yamlv3.NewDecoder(bytes.NewReader(data))
	for {
		var document map[string]any
		if err := decoder.Decode(&document); err != nil {
			if err == io.EOF {
				break
			}
			fatalf("decode CRD bundle: %v", err)
		}
		metadata, _ := document["metadata"].(map[string]any)
		name, _ := metadata["name"].(string)
		cr, ok := wanted[name]
		if !ok {
			continue
		}
		versions := mustSlice(mustMap(document["spec"], "spec")["versions"], "spec.versions")
		for _, rawVersion := range versions {
			version := mustMap(rawVersion, "version")
			if version["name"] == nativeCRDVersion {
				schemas[cr] = mustMap(mustMap(version["schema"], "schema")["openAPIV3Schema"], "openAPIV3Schema")
			}
		}
		if schemas[cr] == nil {
			fatalf("%s has no %s schema", name, nativeCRDVersion)
		}
	}
	for name, cr := range wanted {
		if schemas[cr] == nil {
			fatalf("CRD bundle has no %s", name)
		}
	}
	return schemas
}

// attributeSchema resolves the CRD node an attribute maps to. Attributes with
// no crd_path (Terraform-only fields such as id) resolve to a nil node, which
// inspect reads as "no CRD metadata".
func (s crdSchemas) attributeSchema(item field) map[string]any {
	if item.CRDPath == "" {
		return nil
	}
	return lookup(s.root(item.CR, item.Name), item.CRDPath)
}

// nestedSchema resolves a block field against the block's own CRD node.
func nestedSchema(blockSchema map[string]any, item field) map[string]any {
	if item.CRDPath == "" {
		return nil
	}
	return lookup(blockSchema, item.CRDPath)
}

// blockSchema resolves the CRD node holding a block's nested fields: the object
// itself for a single block, its items for a set.
func (s crdSchemas) blockSchema(item block) map[string]any {
	node := lookup(s.root(item.CR, item.Name), item.CRDPath)
	if item.Collection == "set" {
		return mustMap(node["items"], item.CRDPath+".items")
	}
	return node
}

func (s crdSchemas) root(cr, name string) map[string]any {
	root, ok := s[cr]
	if !ok {
		fatalf("%q maps to unsupported cr %q", name, cr)
	}
	return root
}

func validateMapping(schemas crdSchemas, config mapping) {
	seenNames := map[string]bool{}
	seenGoNames := map[string]bool{}
	registerTopLevel := func(name, goName string) {
		if seenNames[name] {
			fatalf("duplicate top-level Terraform name %q", name)
		}
		seenNames[name] = true
		if seenGoNames[goName] {
			fatalf("duplicate top-level Go name %q", goName)
		}
		seenGoNames[goName] = true
	}

	for _, item := range config.Attributes {
		validateFieldConfig(item)
		registerTopLevel(item.Name, item.GoName)
		validateField(schemas.attributeSchema(item), item)
	}
	for _, item := range config.Blocks {
		registerTopLevel(item.Name, item.GoName)
		if item.Collection != "single" && item.Collection != "set" {
			fatalf("block %q has unsupported collection %q", item.Name, item.Collection)
		}

		expectedType := "object"
		if item.Collection == "set" {
			expectedType = "array"
		}
		node := lookup(schemas.root(item.CR, item.Name), item.CRDPath)
		if actual, _ := node["type"].(string); actual != expectedType {
			fatalf("block %s maps to CRD type %q, expected %q", item.Name, actual, expectedType)
		}
		blockSchema := schemas.blockSchema(item)

		seenNestedNames := map[string]bool{}
		seenNestedGoNames := map[string]bool{}
		for _, nested := range item.Fields {
			validateFieldConfig(nested)
			if nested.CR != "" {
				fatalf("field %q in block %q must not set cr; it is resolved inside %q", nested.Name, item.Name, item.CR)
			}
			if seenNestedNames[nested.Name] {
				fatalf("duplicate Terraform field %q in block %q", nested.Name, item.Name)
			}
			seenNestedNames[nested.Name] = true
			if seenNestedGoNames[nested.GoName] {
				fatalf("duplicate Go field %q in block %q", nested.GoName, item.Name)
			}
			seenNestedGoNames[nested.GoName] = true
			validateField(nestedSchema(blockSchema, nested), nested)
		}
	}
}

func validateFieldConfig(item field) {
	switch item.ValueType {
	case "string", "int64", "normalized_json":
	default:
		fatalf("field %q has unsupported value_type %q", item.Name, item.ValueType)
	}
	switch item.Mode {
	case "required", "optional", "computed", "optional_computed":
	default:
		fatalf("field %q has unsupported mode %q", item.Name, item.Mode)
	}
	if item.RequiresReplace && item.ValueType != "string" {
		fatalf("field %q sets requires_replace with value_type %q", item.Name, item.ValueType)
	}
}

func validateField(node map[string]any, item field) {
	if node == nil {
		return
	}
	info := inspect(node)
	expectedType := baseType(item.ValueType)
	if expectedType == "int64" {
		expectedType = "integer"
	}
	if item.ValueType == "normalized_json" {
		expectedType = "object"
	}
	if info.Type != expectedType {
		fatalf("field %s maps to CRD type %q, expected %q", item.Name, info.Type, expectedType)
	}
}

func render(schemas crdSchemas, config mapping) []byte {
	var out bytes.Buffer
	out.WriteString("// Code generated by cmd/generate-pool-resource; DO NOT EDIT.\n\n")
	out.WriteString("package provider\n\n")
	out.WriteString("import (\n\t\"regexp\"\n\n")
	out.WriteString("\t\"github.com/hashicorp/terraform-plugin-framework-jsontypes/jsontypes\"\n")
	out.WriteString("\t\"github.com/hashicorp/terraform-plugin-framework-validators/int64validator\"\n")
	out.WriteString("\t\"github.com/hashicorp/terraform-plugin-framework-validators/stringvalidator\"\n")
	out.WriteString("\t\"github.com/hashicorp/terraform-plugin-framework/attr\"\n")
	out.WriteString("\t\"github.com/hashicorp/terraform-plugin-framework/resource/schema\"\n")
	out.WriteString("\t\"github.com/hashicorp/terraform-plugin-framework/resource/schema/planmodifier\"\n")
	out.WriteString("\t\"github.com/hashicorp/terraform-plugin-framework/resource/schema/stringplanmodifier\"\n")
	out.WriteString("\t\"github.com/hashicorp/terraform-plugin-framework/schema/validator\"\n")
	out.WriteString("\t\"github.com/hashicorp/terraform-plugin-framework/types\"\n")
	out.WriteString(")\n\n")

	renderModel(&out, "poolResourceModel", config.Attributes, config.Blocks)
	for _, item := range config.Blocks {
		renderModel(&out, item.Model, item.Fields, nil)
	}

	fmt.Fprintf(&out, "func poolResourceSchema() schema.Schema {\n\treturn schema.Schema{Description: %s,\n", strconv.Quote(config.ResourceDescription))
	out.WriteString("\t\tAttributes: map[string]schema.Attribute{\n")
	for _, item := range config.Attributes {
		renderAttribute(&out, schemas.attributeSchema(item), item, "\t\t\t")
	}
	out.WriteString("\t\t},\n\t\tBlocks: map[string]schema.Block{\n")
	for _, item := range config.Blocks {
		blockSchema := schemas.blockSchema(item)
		fmt.Fprintf(&out, "\t\t\t%q: schema.%sNestedBlock{", item.Name, title(item.Collection))
		if item.Collection == "set" {
			out.WriteString("NestedObject: schema.NestedBlockObject{")
		}
		out.WriteString("Attributes: map[string]schema.Attribute{\n")
		for _, nested := range item.Fields {
			renderAttribute(&out, nestedSchema(blockSchema, nested), nested, "\t\t\t\t")
		}
		if item.Collection == "set" {
			out.WriteString("\t\t\t}}},\n")
		} else {
			out.WriteString("\t\t\t}},\n")
		}
	}
	out.WriteString("\t\t},\n\t}\n}\n\n")

	for _, item := range config.Blocks {
		fmt.Fprintf(&out, "func %sObjectType() map[string]attr.Type {\n\treturn map[string]attr.Type{", item.Name)
		for index, nested := range item.Fields {
			if index > 0 {
				out.WriteString(", ")
			}
			fmt.Fprintf(&out, "%q: %s", nested.Name, attrType(nested.ValueType))
		}
		out.WriteString("}\n}\n\n")
	}
	out.WriteString("var dnsLabelRegex = regexp.MustCompile(`^[a-z0-9]([-a-z0-9]*[a-z0-9])?$`)\n")
	return out.Bytes()
}

func renderModel(out *bytes.Buffer, name string, fields []field, blocks []block) {
	fmt.Fprintf(out, "type %s struct {\n", name)
	for _, item := range fields {
		fmt.Fprintf(out, "\t%s %s `tfsdk:%q`\n", item.GoName, modelType(item.ValueType), item.Name)
	}
	for _, item := range blocks {
		valueType := "types.Object"
		if item.Collection == "set" {
			valueType = "types.Set"
		}
		fmt.Fprintf(out, "\t%s %s `tfsdk:%q`\n", item.GoName, valueType, item.Name)
	}
	out.WriteString("}\n\n")
}

func renderAttribute(out *bytes.Buffer, node map[string]any, item field, indent string) {
	info := schemaInfo{}
	if node != nil {
		info = inspect(node)
	}
	if item.Minimum != nil {
		info.Minimum = item.Minimum
	}
	if item.Maximum != nil {
		info.Maximum = item.Maximum
	}
	description := item.Description
	if description == "" {
		description = info.Description
	}
	fmt.Fprintf(out, "%s%q: schema.%sAttribute{", indent, item.Name, attributeType(item.ValueType))
	switch item.Mode {
	case "required":
		out.WriteString("Required: true")
	case "optional":
		out.WriteString("Optional: true")
	case "computed":
		out.WriteString("Computed: true")
	case "optional_computed":
		out.WriteString("Optional: true, Computed: true")
	default:
		fatalf("unsupported mode %q", item.Mode)
	}
	if item.ValueType == "normalized_json" {
		out.WriteString(", CustomType: jsontypes.NormalizedType{}")
	}
	if description != "" {
		fmt.Fprintf(out, ", Description: %s", strconv.Quote(strings.TrimSpace(description)))
	}
	validators := validatorsFor(item, info)
	if len(validators) > 0 {
		fmt.Fprintf(out, ", Validators: []validator.%s{%s}", title(baseType(item.ValueType)), strings.Join(validators, ", "))
	}
	if item.RequiresReplace {
		out.WriteString(", PlanModifiers: []planmodifier.String{stringplanmodifier.RequiresReplace()}")
	}
	out.WriteString("},\n")
}

func validatorsFor(item field, info schemaInfo) []string {
	var result []string
	if item.DNSLabel {
		result = append(result, "stringvalidator.LengthBetween(1, 63)", "stringvalidator.RegexMatches(dnsLabelRegex, \"must be a lowercase DNS label\")")
	}
	if len(info.Enum) > 0 {
		quoted := make([]string, len(info.Enum))
		for index, value := range info.Enum {
			quoted[index] = strconv.Quote(value)
		}
		result = append(result, "stringvalidator.OneOf("+strings.Join(quoted, ", ")+")")
	}
	if info.Minimum != nil && item.ValueType == "int64" {
		if info.Maximum != nil {
			result = append(result, fmt.Sprintf("int64validator.Between(%d, %d)", *info.Minimum, *info.Maximum))
		} else {
			result = append(result, fmt.Sprintf("int64validator.AtLeast(%d)", *info.Minimum))
		}
	}
	return result
}

func lookup(root map[string]any, path string) map[string]any {
	current := root
	for _, part := range strings.Split(path, ".") {
		properties := mustMap(current["properties"], path+".properties")
		current = mustMap(properties[part], path)
	}
	return current
}

func inspect(value map[string]any) schemaInfo {
	info := schemaInfo{}
	if valueType, ok := value["type"].(string); ok {
		info.Type = valueType
	}
	if description, ok := value["description"].(string); ok {
		info.Description = description
	}
	if rawEnum, ok := value["enum"].([]any); ok {
		for _, item := range rawEnum {
			info.Enum = append(info.Enum, fmt.Sprint(item))
		}
		sort.Strings(info.Enum)
	}
	info.Minimum = integerPointer(value["minimum"])
	info.Maximum = integerPointer(value["maximum"])
	return info
}

func integerPointer(value any) *int64 {
	switch typed := value.(type) {
	case float64:
		result := int64(typed)
		return &result
	case int:
		result := int64(typed)
		return &result
	case int64:
		return &typed
	default:
		return nil
	}
}

func modelType(valueType string) string {
	if valueType == "normalized_json" {
		return "jsontypes.Normalized"
	}
	return "types." + title(valueType)
}

func attrType(valueType string) string {
	if valueType == "int64" {
		return "types.Int64Type"
	}
	return "types.StringType"
}

func attributeType(valueType string) string { return title(baseType(valueType)) }
func baseType(valueType string) string {
	if valueType == "normalized_json" {
		return "string"
	}
	return valueType
}
func title(value string) string {
	if value == "int64" {
		return "Int64"
	}
	return strings.ToUpper(value[:1]) + value[1:]
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
func fatalf(format string, args ...any) {
	fmt.Fprintf(os.Stderr, format+"\n", args...)
	os.Exit(1)
}
