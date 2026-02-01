terraform {
  required_version = ">= 1.0"

  required_providers {
    aws = {
      source  = "hashicorp/aws"
      version = "~> 5.0"
    }
  }

  backend "s3" {
    bucket         = "cua-terraform-state"
    key            = "ssm-feature-flags/terraform.tfstate"
    region         = "us-west-2"
    encrypt        = true
    dynamodb_table = "cua-terraform-locks"
  }
}

provider "aws" {
  region = var.aws_region

  default_tags {
    tags = {
      Project     = "cua"
      ManagedBy   = "terraform"
      Environment = var.environment
    }
  }
}
