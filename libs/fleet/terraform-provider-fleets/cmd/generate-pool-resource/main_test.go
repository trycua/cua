package main

import (
	"bytes"
	"go/format"
	"os"
	"os/exec"
	"strings"
	"testing"
)

func TestValidateMappingAcceptsValidMapping(t *testing.T) {
	stderr, err := runValidationHelper(t, "valid")
	if err != nil {
		t.Fatalf("validateMapping() failed: %v\nstderr:\n%s", err, stderr)
	}
}

func TestValidateMappingRejectsInvalidMapping(t *testing.T) {
	tests := []struct {
		name       string
		scenario   string
		wantStderr string
	}{
		{name: "unsupported value type", scenario: "unsupported_value_type", wantStderr: `field "name" has unsupported value_type "float"`},
		{name: "unsupported mode", scenario: "unsupported_mode", wantStderr: `field "name" has unsupported mode "write_only"`},
		{name: "requires replace on non-string", scenario: "requires_replace_int64", wantStderr: `field "replicas" sets requires_replace with value_type "int64"`},
		{name: "unsupported collection", scenario: "unsupported_collection", wantStderr: `block "service" has unsupported collection "map"`},
		{name: "duplicate top-level name", scenario: "duplicate_top_level_name", wantStderr: `duplicate top-level Terraform name "name"`},
		{name: "duplicate top-level Go name", scenario: "duplicate_top_level_go_name", wantStderr: `duplicate top-level Go name "Name"`},
		{name: "duplicate block name", scenario: "duplicate_block_name", wantStderr: `duplicate top-level Terraform name "service"`},
		{name: "duplicate nested name", scenario: "duplicate_nested_name", wantStderr: `duplicate Terraform field "name" in block "service"`},
		{name: "duplicate nested Go name", scenario: "duplicate_nested_go_name", wantStderr: `duplicate Go field "Name" in block "service"`},
		{name: "missing CRD path", scenario: "missing_crd_path", wantStderr: `spec.missing is not an object`},
		{name: "wrong CRD type", scenario: "wrong_crd_type", wantStderr: `field replicas maps to CRD type "string", expected "integer"`},
		{name: "unknown attribute CR", scenario: "unknown_attribute_cr", wantStderr: `"replicas" maps to unsupported cr "sandbox"`},
		{name: "missing attribute CR", scenario: "missing_attribute_cr", wantStderr: `"replicas" maps to unsupported cr ""`},
		{name: "unknown block CR", scenario: "unknown_block_cr", wantStderr: `"service" maps to unsupported cr "sandbox"`},
		{name: "nested CR", scenario: "nested_cr", wantStderr: `field "name" in block "service" must not set cr`},
		{name: "path in the other CR", scenario: "path_in_other_cr", wantStderr: `spec.vmTemplate.services is not an object`},
		{name: "wrong list element type", scenario: "wrong_list_element_type", wantStderr: `field command maps to CRD element type "integer", expected "string"`},
		{name: "wrong map element type", scenario: "wrong_map_element_type", wantStderr: `field env maps to CRD element type "", expected "string"`},
	}

	for _, test := range tests {
		t.Run(test.name, func(t *testing.T) {
			stderr, err := runValidationHelper(t, test.scenario)
			if err == nil {
				t.Fatalf("validateMapping() succeeded, want failure")
			}
			if !strings.Contains(stderr, test.wantStderr) {
				t.Fatalf("stderr = %q, want it to contain %q", stderr, test.wantStderr)
			}
		})
	}
}

func TestRenderIsDeterministic(t *testing.T) {
	schemas, config := validationFixture("valid")
	first := render(schemas, config)
	second := render(schemas, config)
	if !bytes.Equal(first, second) {
		t.Fatal("render() produced different output for identical input")
	}
}

func TestRenderCollectionAndBoolTypes(t *testing.T) {
	schemas, config := validationFixture("valid")
	config.Attributes = append(config.Attributes,
		field{Name: "command", GoName: "Command", ValueType: "string_list", Mode: "optional", CR: templateCR, CRDPath: "spec.vmTemplate.command"},
		field{Name: "env", GoName: "Env", ValueType: "string_map", Mode: "optional", CR: templateCR, CRDPath: "spec.vmTemplate.env"},
		field{Name: "claim_secrets", GoName: "ClaimSecrets", ValueType: "bool", Mode: "optional", CR: templateCR, CRDPath: "spec.vmTemplate.claimSecrets"},
		field{Name: "ports", GoName: "Ports", ValueType: "int64_list", Mode: "optional", CR: templateCR, CRDPath: "spec.vmTemplate.ports"},
	)
	config.Blocks[0].Collection = "list"
	validateMapping(schemas, config)
	output, err := format.Source(render(schemas, config))
	if err != nil {
		t.Fatalf("generated source does not parse: %v", err)
	}
	// gofmt aligns struct fields and map keys; compare with single spaces.
	generated := strings.Join(strings.Fields(string(output)), " ")
	for _, want := range []string{
		"Command types.List `tfsdk:\"command\"`",
		"Env types.Map `tfsdk:\"env\"`",
		"ClaimSecrets types.Bool `tfsdk:\"claim_secrets\"`",
		"Services types.List `tfsdk:\"service\"`",
		`"command": schema.ListAttribute{Optional: true, ElementType: types.StringType}`,
		`"env": schema.MapAttribute{Optional: true, ElementType: types.StringType}`,
		`"claim_secrets": schema.BoolAttribute{Optional: true}`,
		`"ports": schema.ListAttribute{Optional: true, ElementType: types.Int64Type, Validators: []validator.List{listvalidator.ValueInt64sAre(int64validator.Between(1, 65535))}}`,
		`"service": schema.ListNestedBlock{NestedObject: schema.NestedBlockObject{`,
		`"github.com/hashicorp/terraform-plugin-framework-validators/listvalidator"`,
	} {
		if !strings.Contains(generated, want) {
			t.Errorf("generated source is missing %s\n%s", want, output)
		}
	}
	if strings.Contains(generated, "jsontypes") {
		t.Error("generated source imports jsontypes without using it")
	}
}

func TestReadCRDSchemasFindsBothNativeCRDs(t *testing.T) {
	schemas := readCRDSchemas("../../../../clusters/base/osgym/crd.yaml")
	if lookup(schemas.root(warmPoolCR, "replicas"), "spec.replicas")["type"] != "integer" {
		t.Fatal("warm pool schema does not expose spec.replicas")
	}
	if lookup(schemas.root(templateCR, "container_disk_image"), "spec.vmTemplate.containerDiskImage")["type"] != "string" {
		t.Fatal("template schema does not expose spec.vmTemplate.containerDiskImage")
	}
}

func TestValidationHelperProcess(t *testing.T) {
	scenario := os.Getenv("GO_GENERATOR_VALIDATION_SCENARIO")
	if scenario == "" {
		return
	}
	schemas, config := validationFixture(scenario)
	validateMapping(schemas, config)
}

func runValidationHelper(t *testing.T, scenario string) (string, error) {
	t.Helper()
	command := exec.Command(os.Args[0], "-test.run=^TestValidationHelperProcess$")
	command.Env = append(os.Environ(), "GO_GENERATOR_VALIDATION_SCENARIO="+scenario)
	output, err := command.CombinedOutput()
	return string(output), err
}

func validationFixture(scenario string) (crdSchemas, mapping) {
	warmPool := map[string]any{
		"properties": map[string]any{
			"spec": map[string]any{
				"type": "object",
				"properties": map[string]any{
					"replicas": map[string]any{"type": "integer"},
				},
			},
		},
	}
	template := map[string]any{
		"properties": map[string]any{
			"spec": map[string]any{
				"type": "object",
				"properties": map[string]any{
					"vmTemplate": map[string]any{
						"type": "object",
						"properties": map[string]any{
							"command":      map[string]any{"type": "array", "items": map[string]any{"type": "string"}},
							"env":          map[string]any{"type": "object", "additionalProperties": map[string]any{"type": "string"}},
							"claimSecrets": map[string]any{"type": "boolean"},
							"ports":        map[string]any{"type": "array", "items": map[string]any{"type": "integer", "minimum": 1.0, "maximum": 65535.0}},
							"services": map[string]any{
								"type": "array",
								"items": map[string]any{
									"type": "object",
									"properties": map[string]any{
										"name":       map[string]any{"type": "string"},
										"targetPort": map[string]any{"type": "integer"},
									},
								},
							},
						},
					},
				},
			},
		},
	}
	schemas := crdSchemas{warmPoolCR: warmPool, templateCR: template}
	config := mapping{
		Attributes: []field{
			{Name: "name", GoName: "Name", ValueType: "string", Mode: "required", RequiresReplace: true},
			{Name: "replicas", GoName: "Replicas", ValueType: "int64", Mode: "required", CR: warmPoolCR, CRDPath: "spec.replicas"},
		},
		Blocks: []block{{
			Name: "service", GoName: "Services", Model: "serviceModel", Collection: "set",
			CR: templateCR, CRDPath: "spec.vmTemplate.services",
			Fields: []field{{Name: "name", GoName: "Name", ValueType: "string", Mode: "required", CRDPath: "name"}},
		}},
	}

	switch scenario {
	case "valid":
	case "unsupported_value_type":
		config.Attributes[0].ValueType = "float"
	case "unsupported_mode":
		config.Attributes[0].Mode = "write_only"
	case "requires_replace_int64":
		config.Attributes[1].RequiresReplace = true
	case "unsupported_collection":
		config.Blocks[0].Collection = "map"
	case "duplicate_top_level_name":
		config.Blocks[0].Name = "name"
	case "duplicate_top_level_go_name":
		config.Blocks[0].GoName = "Name"
	case "duplicate_block_name":
		duplicate := config.Blocks[0]
		duplicate.GoName = "OtherServices"
		duplicate.Model = "otherServiceModel"
		config.Blocks = append(config.Blocks, duplicate)
	case "duplicate_nested_name":
		config.Blocks[0].Fields = append(config.Blocks[0].Fields, field{Name: "name", GoName: "OtherName", ValueType: "string", Mode: "required", CRDPath: "name"})
	case "duplicate_nested_go_name":
		config.Blocks[0].Fields = append(config.Blocks[0].Fields, field{Name: "target_port", GoName: "Name", ValueType: "int64", Mode: "required", CRDPath: "targetPort"})
	case "missing_crd_path":
		config.Attributes[1].CRDPath = "spec.missing"
	case "wrong_crd_type":
		warmPoolSpec(warmPool)["replicas"].(map[string]any)["type"] = "string"
	case "unknown_attribute_cr":
		config.Attributes[1].CR = "sandbox"
	case "missing_attribute_cr":
		config.Attributes[1].CR = ""
	case "unknown_block_cr":
		config.Blocks[0].CR = "sandbox"
	case "nested_cr":
		config.Blocks[0].Fields[0].CR = templateCR
	case "wrong_list_element_type":
		config.Attributes = append(config.Attributes, field{Name: "command", GoName: "Command", ValueType: "string_list", Mode: "optional", CR: templateCR, CRDPath: "spec.vmTemplate.command"})
		vmTemplateProperties(template)["command"].(map[string]any)["items"] = map[string]any{"type": "integer"}
	case "wrong_map_element_type":
		config.Attributes = append(config.Attributes, field{Name: "env", GoName: "Env", ValueType: "string_map", Mode: "optional", CR: templateCR, CRDPath: "spec.vmTemplate.env"})
		delete(vmTemplateProperties(template)["env"].(map[string]any), "additionalProperties")
	case "path_in_other_cr":
		config.Blocks[0].CR = warmPoolCR
	default:
		panic("unknown validation scenario: " + scenario)
	}
	return schemas, config
}

func vmTemplateProperties(root map[string]any) map[string]any {
	return root["properties"].(map[string]any)["spec"].(map[string]any)["properties"].(map[string]any)["vmTemplate"].(map[string]any)["properties"].(map[string]any)
}

func warmPoolSpec(root map[string]any) map[string]any {
	return root["properties"].(map[string]any)["spec"].(map[string]any)["properties"].(map[string]any)
}
