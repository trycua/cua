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

## Requirements

- Terraform >= 1.0
- AWS Provider ~> 5.0
- AWS credentials configured
