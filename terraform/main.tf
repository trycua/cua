# Private S3 bucket for CUA docs web scrape
resource "aws_s3_bucket" "cua_docs_web_scrape" {
  bucket = var.bucket_name

  tags = {
    Name        = var.bucket_name
    Environment = var.environment
    Purpose     = "CUA documentation web scraping storage"
  }
}

# Block all public access to the bucket
resource "aws_s3_bucket_public_access_block" "cua_docs_web_scrape" {
  bucket = aws_s3_bucket.cua_docs_web_scrape.id

  block_public_acls       = true
  block_public_policy     = true
  ignore_public_acls      = true
  restrict_public_buckets = true
}

# Enable versioning for data protection
resource "aws_s3_bucket_versioning" "cua_docs_web_scrape" {
  bucket = aws_s3_bucket.cua_docs_web_scrape.id

  versioning_configuration {
    status = "Enabled"
  }
}

# Enable server-side encryption by default
resource "aws_s3_bucket_server_side_encryption_configuration" "cua_docs_web_scrape" {
  bucket = aws_s3_bucket.cua_docs_web_scrape.id

  rule {
    apply_server_side_encryption_by_default {
      sse_algorithm = "AES256"
    }
    bucket_key_enabled = true
  }
}

# Bucket policy to enforce SSL/TLS connections only
resource "aws_s3_bucket_policy" "cua_docs_web_scrape" {
  bucket = aws_s3_bucket.cua_docs_web_scrape.id

  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [
      {
        Sid       = "EnforceSSLOnly"
        Effect    = "Deny"
        Principal = "*"
        Action    = "s3:*"
        Resource = [
          aws_s3_bucket.cua_docs_web_scrape.arn,
          "${aws_s3_bucket.cua_docs_web_scrape.arn}/*"
        ]
        Condition = {
          Bool = {
            "aws:SecureTransport" = "false"
          }
        }
      }
    ]
  })

  depends_on = [aws_s3_bucket_public_access_block.cua_docs_web_scrape]
}
