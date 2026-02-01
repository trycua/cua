# GitHub OIDC Provider
# This creates or references the GitHub OIDC identity provider in AWS
data "aws_iam_openid_connect_provider" "github" {
  count = var.create_oidc_provider ? 0 : 1
  url   = "https://token.actions.githubusercontent.com"
}

resource "aws_iam_openid_connect_provider" "github" {
  count = var.create_oidc_provider ? 1 : 0

  url             = "https://token.actions.githubusercontent.com"
  client_id_list  = ["sts.amazonaws.com"]
  thumbprint_list = ["6938fd4d98bab03faadb97b34396831e3780aea1", "1c58a3a8518e8759bf075b76b750d4f2df264fcd"]

  tags = {
    Name = "github-actions-oidc"
  }
}

locals {
  oidc_provider_arn = var.create_oidc_provider ? aws_iam_openid_connect_provider.github[0].arn : data.aws_iam_openid_connect_provider.github[0].arn

  # Build the subject claim conditions for allowed branches
  branch_conditions = [
    for branch in var.allowed_branches : "repo:${var.github_org}/${var.github_repo}:ref:refs/heads/${branch}"
  ]
}

# IAM Role for GitHub Actions to deploy SSM parameters
resource "aws_iam_role" "github_actions_ssm_deploy" {
  name = "github-actions-ssm-deploy"

  assume_role_policy = jsonencode({
    Version = "2012-10-17"
    Statement = [
      {
        Effect = "Allow"
        Principal = {
          Federated = local.oidc_provider_arn
        }
        Action = "sts:AssumeRoleWithWebIdentity"
        Condition = {
          StringEquals = {
            "token.actions.githubusercontent.com:aud" = "sts.amazonaws.com"
          }
          StringLike = {
            "token.actions.githubusercontent.com:sub" = local.branch_conditions
          }
        }
      }
    ]
  })

  tags = {
    Name    = "github-actions-ssm-deploy"
    Purpose = "Allow GitHub Actions to deploy SSM feature flags"
  }
}

# IAM Policy for SSM Parameter Store access
resource "aws_iam_role_policy" "ssm_deploy" {
  name = "ssm-feature-flags-deploy"
  role = aws_iam_role.github_actions_ssm_deploy.id

  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [
      {
        Sid    = "SSMParameterAccess"
        Effect = "Allow"
        Action = [
          "ssm:PutParameter",
          "ssm:GetParameter",
          "ssm:GetParameters",
          "ssm:GetParametersByPath",
          "ssm:DeleteParameter",
          "ssm:DescribeParameters",
          "ssm:ListTagsForResource",
          "ssm:AddTagsToResource",
          "ssm:RemoveTagsFromResource"
        ]
        Resource = [
          "arn:aws:ssm:${var.aws_region}:*:parameter/cua/feature-flags/*",
          "arn:aws:ssm:${var.aws_region}:*:parameter/cua/rollout/*"
        ]
      },
      {
        Sid    = "SSMDescribeParameters"
        Effect = "Allow"
        Action = [
          "ssm:DescribeParameters"
        ]
        Resource = "*"
      },
      {
        Sid    = "S3TerraformState"
        Effect = "Allow"
        Action = [
          "s3:GetObject",
          "s3:PutObject",
          "s3:DeleteObject",
          "s3:ListBucket"
        ]
        Resource = [
          "arn:aws:s3:::cua-terraform-state",
          "arn:aws:s3:::cua-terraform-state/ssm-feature-flags/*"
        ]
      },
      {
        Sid    = "DynamoDBTerraformLock"
        Effect = "Allow"
        Action = [
          "dynamodb:GetItem",
          "dynamodb:PutItem",
          "dynamodb:DeleteItem"
        ]
        Resource = "arn:aws:dynamodb:${var.aws_region}:*:table/cua-terraform-locks"
      }
    ]
  })
}
