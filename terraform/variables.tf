variable "aws_region" {
  description = "AWS region for the S3 bucket"
  type        = string
  default     = "us-east-1"
}

variable "bucket_name" {
  description = "Name of the S3 bucket"
  type        = string
  default     = "cua-docs-web-scrape"
}

variable "environment" {
  description = "Environment tag for the resources"
  type        = string
  default     = "production"
}
