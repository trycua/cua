variable "aws_region" {
  description = "AWS region to deploy resources"
  type        = string
  default     = "us-west-2"
}

variable "environment" {
  description = "Environment name (e.g., production, staging)"
  type        = string
  default     = "production"
}

variable "github_org" {
  description = "GitHub organization name"
  type        = string
  default     = "trycua"
}

variable "github_repo" {
  description = "GitHub repository name"
  type        = string
  default     = "cua"
}

variable "allowed_branches" {
  description = "List of branch patterns allowed to assume the OIDC role"
  type        = list(string)
  default     = ["claude/canary-deploy-incus-vm-ZKG5g"]
}

variable "incus_rollout_orgs" {
  description = "List of organization IDs enabled for incus rollout"
  type        = list(string)
  default     = ["e0988a43e7a5"]
}

variable "feature_flags" {
  description = "Map of feature flags and their enabled status"
  type        = map(bool)
  default = {
    incus_enabled = true
  }
}

variable "create_oidc_provider" {
  description = "Whether to create the GitHub OIDC provider (set to false if it already exists)"
  type        = bool
  default     = true
}
