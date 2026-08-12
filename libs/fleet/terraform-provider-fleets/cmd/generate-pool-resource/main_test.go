package main

import (
	"bytes"
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
		{name: "unsupported value type", scenario: "unsupported_value_type", wantStderr: `field "name" has unsupported value_type "bool"`},
		{name: "unsupported mode", scenario: "unsupported_mode", wantStderr: `field "name" has unsupported mode "write_only"`},
		{name: "requires replace on non-string", scenario: "requires_replace_int64", wantStderr: `field "replicas" sets requires_replace with value_type "int64"`},
		{name: "unsupported collection", scenario: "unsupported_collection", wantStderr: `block "service" has unsupported collection "list"`},
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
		config.Attributes[0].ValueType = "bool"
	case "unsupported_mode":
		config.Attributes[0].Mode = "write_only"
	case "requires_replace_int64":
		config.Attributes[1].RequiresReplace = true
	case "unsupported_collection":
		config.Blocks[0].Collection = "list"
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
	case "path_in_other_cr":
		config.Blocks[0].CR = warmPoolCR
	default:
		panic("unknown validation scenario: " + scenario)
	}
	return schemas, config
}

func warmPoolSpec(root map[string]any) map[string]any {
	return root["properties"].(map[string]any)["spec"].(map[string]any)["properties"].(map[string]any)
}
