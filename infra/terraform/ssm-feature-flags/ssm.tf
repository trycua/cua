# SSM Parameters for Feature Flags
# These parameters control feature flagging across the CUA platform

# Feature flag: Enable/disable incus feature globally
resource "aws_ssm_parameter" "feature_flag_incus_enabled" {
  name        = "/cua/feature-flags/incus/enabled"
  description = "Global feature flag to enable/disable incus functionality"
  type        = "String"
  value       = var.feature_flags["incus_enabled"] ? "true" : "false"

  tags = {
    Name        = "incus-enabled"
    FeatureFlag = "true"
    Category    = "incus"
  }
}

# SSM Parameters for Incus Rollout Configuration
# These parameters control which organizations have incus rollout enabled

# Store the list of organization IDs enabled for incus rollout as a StringList
resource "aws_ssm_parameter" "incus_rollout_orgs" {
  name        = "/cua/rollout/incus/enabled-orgs"
  description = "List of organization IDs enabled for incus rollout"
  type        = "StringList"
  value       = join(",", var.incus_rollout_orgs)

  tags = {
    Name     = "incus-rollout-orgs"
    Rollout  = "true"
    Category = "incus"
  }
}

# Individual org-level rollout flags for faster lookups
resource "aws_ssm_parameter" "incus_rollout_org" {
  for_each = toset(var.incus_rollout_orgs)

  name        = "/cua/rollout/incus/org/${each.value}"
  description = "Incus rollout flag for organization ${each.value}"
  type        = "String"
  value       = "enabled"

  tags = {
    Name           = "incus-rollout-org-${each.value}"
    Rollout        = "true"
    Category       = "incus"
    OrganizationId = each.value
  }
}

# Rollout metadata parameter
resource "aws_ssm_parameter" "incus_rollout_metadata" {
  name        = "/cua/rollout/incus/metadata"
  description = "Metadata for incus rollout configuration"
  type        = "String"
  value = jsonencode({
    version         = "1.0"
    last_updated    = timestamp()
    enabled_org_ids = var.incus_rollout_orgs
    rollout_stage   = "canary"
  })

  tags = {
    Name     = "incus-rollout-metadata"
    Rollout  = "true"
    Category = "incus"
  }

  lifecycle {
    ignore_changes = [value]
  }
}
