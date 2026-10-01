package provider

import (
	"testing"

	"github.com/hashicorp/terraform-plugin-framework/attr"
	"github.com/hashicorp/terraform-plugin-framework/types"
)

func TestValidatePoolScalingModeDuringPlan(t *testing.T) {
	autoscaling := testValidationAutoscalingValue()

	tests := []struct {
		name        string
		replicas    types.Int64
		autoscaling types.Object
		wantError   bool
	}{
		{name: "static", replicas: types.Int64Value(2), autoscaling: types.ObjectNull(autoscalingObjectType())},
		{name: "autoscaled", replicas: types.Int64Null(), autoscaling: autoscaling},
		{name: "both", replicas: types.Int64Value(2), autoscaling: autoscaling, wantError: true},
		{name: "neither", replicas: types.Int64Null(), autoscaling: types.ObjectNull(autoscalingObjectType()), wantError: true},
		{name: "unknown replicas", replicas: types.Int64Unknown(), autoscaling: types.ObjectNull(autoscalingObjectType())},
		{name: "unknown autoscaling", replicas: types.Int64Null(), autoscaling: types.ObjectUnknown(autoscalingObjectType())},
	}

	for _, test := range tests {
		t.Run(test.name, func(t *testing.T) {
			config := poolResourceModel{Replicas: test.replicas, Autoscaling: test.autoscaling}
			err := validatePoolScalingModeDuringPlan(config)
			if (err != nil) != test.wantError {
				t.Fatalf("validatePoolScalingModeDuringPlan() error = %v, wantError %t", err, test.wantError)
			}
		})
	}
}

func TestValidatePoolScalingModeAtApply(t *testing.T) {
	autoscaling := testValidationAutoscalingValue()
	nullAutoscaling := types.ObjectNull(autoscalingObjectType())

	tests := []struct {
		name      string
		config    poolResourceModel
		plan      poolResourceModel
		wantError string
	}{
		{
			name:   "static",
			config: poolResourceModel{Replicas: types.Int64Value(2), Autoscaling: nullAutoscaling},
			plan:   poolResourceModel{Replicas: types.Int64Value(2), Autoscaling: nullAutoscaling},
		},
		{
			name:   "autoscaled with computed live replicas",
			config: poolResourceModel{Replicas: types.Int64Null(), Autoscaling: autoscaling},
			plan:   poolResourceModel{Replicas: types.Int64Value(15), Autoscaling: autoscaling},
		},
		{
			name:      "both",
			config:    poolResourceModel{Replicas: types.Int64Value(2), Autoscaling: autoscaling},
			plan:      poolResourceModel{Replicas: types.Int64Value(2), Autoscaling: autoscaling},
			wantError: poolScalingModeExactlyOneMessage,
		},
		{
			name:      "neither",
			config:    poolResourceModel{Replicas: types.Int64Null(), Autoscaling: nullAutoscaling},
			plan:      poolResourceModel{Replicas: types.Int64Unknown(), Autoscaling: nullAutoscaling},
			wantError: poolScalingModeExactlyOneMessage,
		},
		{
			name:      "unknown replicas resolve present beside autoscaling",
			config:    poolResourceModel{Replicas: types.Int64Unknown(), Autoscaling: autoscaling},
			plan:      poolResourceModel{Replicas: types.Int64Value(2), Autoscaling: autoscaling},
			wantError: poolScalingModeExactlyOneMessage,
		},
		{
			name:      "unknown replicas resolve null without autoscaling",
			config:    poolResourceModel{Replicas: types.Int64Unknown(), Autoscaling: nullAutoscaling},
			plan:      poolResourceModel{Replicas: types.Int64Null(), Autoscaling: nullAutoscaling},
			wantError: poolScalingModeExactlyOneMessage,
		},
		{
			name:      "unknown replicas remain unresolved",
			config:    poolResourceModel{Replicas: types.Int64Unknown(), Autoscaling: nullAutoscaling},
			plan:      poolResourceModel{Replicas: types.Int64Unknown(), Autoscaling: nullAutoscaling},
			wantError: poolScalingModeUnknownAtApplyMessage,
		},
	}

	for _, test := range tests {
		t.Run(test.name, func(t *testing.T) {
			err := validatePoolScalingModeAtApply(test.config, test.plan)
			if test.wantError == "" {
				if err != nil {
					t.Fatalf("validatePoolScalingModeAtApply() error = %v", err)
				}
				return
			}
			if err == nil || err.Error() != test.wantError {
				t.Fatalf("validatePoolScalingModeAtApply() error = %v, want %q", err, test.wantError)
			}
		})
	}
}

func testValidationAutoscalingValue() types.Object {
	return types.ObjectValueMust(autoscalingObjectType(), map[string]attr.Value{
		"min_pool_size":     types.Int64Value(0),
		"initial_pool_size": types.Int64Value(1),
		"max_pool_size":     types.Int64Value(5),
	})
}
