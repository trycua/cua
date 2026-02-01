# SSM Feature Flags Terraform Infrastructure

This Terraform project manages AWS SSM Parameter Store parameters for feature flags and rollout configurations, along with the GitHub Actions OIDC role required to deploy them.

## Overview

This infrastructure creates:

1. **GitHub OIDC Provider** - Enables GitHub Actions to authenticate with AWS using OIDC
2. **IAM Role** - Allows specific branches to assume the role and manage SSM parameters
3. **SSM Parameters** - Feature flags and rollout configuration for the CUA platform

## SSM Parameters Created

| Parameter Path | Type | Description |
|----------------|------|-------------|
| `/cua/feature-flags/incus/enabled` | String | Global feature flag for incus |
| `/cua/rollout/incus/enabled-orgs` | StringList | List of org IDs enabled for incus |
| `/cua/rollout/incus/org/{org_id}` | String | Per-org rollout flag for fast lookups |
| `/cua/rollout/incus/metadata` | String (JSON) | Rollout metadata and version info |

## OIDC Role Configuration

The IAM role `github-actions-ssm-deploy` is configured to be assumable only from specific branches:

- `claude/canary-deploy-incus-vm-ZKG5g`

This is enforced via the OIDC subject claim condition in the trust policy.

## Usage

### Prerequisites

1. AWS account with appropriate permissions
2. S3 bucket for Terraform state: `cua-terraform-state`
3. DynamoDB table for state locking: `cua-terraform-locks`
4. GitHub repository secret `AWS_ACCOUNT_ID` set

### Local Development

```bash
# Copy and configure variables
cp terraform.tfvars.example terraform.tfvars
# Edit terraform.tfvars with your values

# Initialize Terraform
terraform init

# Plan changes
terraform plan

# Apply changes
terraform apply
```

### GitHub Actions Deployment

The workflow automatically runs on push to the allowed branch. You can also trigger it manually:

1. Go to Actions > "CD: Terraform SSM Feature Flags"
2. Click "Run workflow"
3. Select the action: `plan`, `apply`, or `destroy`

## Adding New Organizations to Rollout

To add a new organization to the incus rollout:

1. Add the org ID to the `incus_rollout_orgs` variable in `terraform.tfvars`
2. Commit and push to the allowed branch
3. The workflow will automatically apply the changes

## Reading Parameters in Application Code

```python
import boto3
import json

ssm = boto3.client('ssm')

# Check if incus is enabled globally
response = ssm.get_parameter(Name='/cua/feature-flags/incus/enabled')
incus_enabled = response['Parameter']['Value'] == 'true'

# Check if org is in rollout
def is_org_in_incus_rollout(org_id: str) -> bool:
    try:
        response = ssm.get_parameter(Name=f'/cua/rollout/incus/org/{org_id}')
        return response['Parameter']['Value'] == 'enabled'
    except ssm.exceptions.ParameterNotFound:
        return False

# Get all enabled orgs
response = ssm.get_parameter(Name='/cua/rollout/incus/enabled-orgs')
enabled_orgs = response['Parameter']['Value'].split(',')
```

## Security Considerations

- The OIDC role only allows access to specific SSM parameter paths
- Branch restrictions prevent unauthorized deployments
- All parameters are tagged for auditing and cost allocation
