# CUA Docs Web Scrape S3 Bucket

Terraform configuration for a private AWS S3 bucket used for storing CUA documentation web scrape data.

## Features

- **Private by default**: All public access is blocked
- **Encryption**: Server-side encryption with AES-256
- **Versioning**: Enabled for data protection
- **SSL/TLS enforced**: Bucket policy denies non-HTTPS requests

## Usage

### Initialize Terraform

```bash
terraform init
```

### Preview changes

```bash
terraform plan
```

### Apply changes

```bash
terraform apply
```

### Destroy resources

```bash
terraform destroy
```

## Variables

| Name | Description | Default |
|------|-------------|---------|
| `aws_region` | AWS region for the S3 bucket | `us-east-1` |
| `bucket_name` | Name of the S3 bucket | `cua-docs-web-scrape` |
| `environment` | Environment tag for resources | `production` |

## Outputs

| Name | Description |
|------|-------------|
| `bucket_id` | The name of the bucket |
| `bucket_arn` | The ARN of the bucket |
| `bucket_region` | The AWS region the bucket is in |

## CI/CD

This project includes a GitHub Actions workflow (`.github/workflows/terraform.yml`) that automates:

### On Pull Requests
- **Format Check**: Validates Terraform formatting
- **Validate**: Runs `terraform validate`
- **Plan**: Creates and posts execution plan as a PR comment

### On Push to Main
- **Apply**: Automatically applies changes (requires `production` environment approval)

### Required Secrets

Configure these secrets in your GitHub repository settings:

| Secret | Description |
|--------|-------------|
| `AWS_ACCESS_KEY_ID` | AWS access key with S3 permissions |
| `AWS_SECRET_ACCESS_KEY` | AWS secret access key |

### Environment Setup

Create a `production` environment in GitHub repository settings for apply approval gates.

## Requirements

- Terraform >= 1.0
- AWS Provider ~> 5.0
- AWS credentials configured
