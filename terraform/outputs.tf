output "bucket_id" {
  description = "The name of the bucket"
  value       = aws_s3_bucket.cua_docs_web_scrape.id
}

output "bucket_arn" {
  description = "The ARN of the bucket"
  value       = aws_s3_bucket.cua_docs_web_scrape.arn
}

output "bucket_region" {
  description = "The AWS region the bucket is in"
  value       = aws_s3_bucket.cua_docs_web_scrape.region
}
