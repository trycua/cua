# Outputs for the SSM Feature Flags Infrastructure

output "oidc_role_arn" {
  description = "ARN of the IAM role for GitHub Actions to assume"
  value       = aws_iam_role.github_actions_ssm_deploy.arn
}

output "oidc_role_name" {
  description = "Name of the IAM role for GitHub Actions"
  value       = aws_iam_role.github_actions_ssm_deploy.name
}

output "feature_flag_incus_enabled_arn" {
  description = "ARN of the incus enabled feature flag SSM parameter"
  value       = aws_ssm_parameter.feature_flag_incus_enabled.arn
}

output "incus_rollout_orgs_arn" {
  description = "ARN of the incus rollout orgs SSM parameter"
  value       = aws_ssm_parameter.incus_rollout_orgs.arn
}

output "incus_rollout_org_arns" {
  description = "Map of organization IDs to their SSM parameter ARNs"
  value       = { for k, v in aws_ssm_parameter.incus_rollout_org : k => v.arn }
}

output "allowed_branches" {
  description = "List of branches allowed to assume the OIDC role"
  value       = var.allowed_branches
}

output "incus_rollout_org_ids" {
  description = "List of organization IDs enabled for incus rollout"
  value       = var.incus_rollout_orgs
}
