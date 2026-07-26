# =============================================================================
# Core Outputs
# =============================================================================

output "pages_bucket_name" {
  description = "S3 bucket for wiki page content"
  value       = aws_s3_bucket.pages.id
}

output "graph_table_name" {
  description = "DynamoDB table for graph metadata"
  value       = aws_dynamodb_table.graph.name
}

output "vectors_bucket_name" {
  description = "S3 Vectors bucket name (must be created via AWS CLI)"
  value       = local.vectors_bucket
}

output "vectors_index_name" {
  description = "S3 Vectors index name"
  value       = var.vectors_index_name
}

output "logs_bucket_name" {
  description = "S3 bucket for agent session logs"
  value       = aws_s3_bucket.logs.id
}

output "mind_palace_policy_arn" {
  description = "IAM policy ARN for Mind Palace runtime access"
  value       = aws_iam_policy.mind_palace.arn
}

# =============================================================================
# Dreaming Outputs
# =============================================================================

output "ecr_repository_url" {
  description = "ECR repository URL for pushing the dream image"
  value       = aws_ecr_repository.dream.repository_url
}

output "task_definition_arn" {
  description = "ECS task definition ARN"
  value       = aws_ecs_task_definition.dream.arn
}

output "cluster_arn" {
  description = "ECS cluster ARN used by dreaming process"
  value       = local.cluster_arn
}

output "schedule_arn" {
  description = "EventBridge schedule ARN"
  value       = aws_scheduler_schedule.dream.arn
}

output "log_group_name" {
  description = "CloudWatch log group for dreaming process"
  value       = aws_cloudwatch_log_group.dream.name
}

# =============================================================================
# Configuration Helper
# =============================================================================

output "rust_config" {
  description = "Paste this into your Rust config"
  value       = <<-EOT
    S3Config { bucket_name: "${aws_s3_bucket.pages.id}", region: "${var.region}", prefix: "v1" }
    DynamoConfig { table_name: "${aws_dynamodb_table.graph.name}", region: "${var.region}" }
    S3VectorsConfig { bucket_name: "${local.vectors_bucket}", index_name: "${var.vectors_index_name}", region: "${var.region}" }
    BedrockConfig { model_id: "amazon.titan-embed-text-v2:0", region: "${var.region}" }
    LogsConfig { bucket_name: "${aws_s3_bucket.logs.id}", region: "${var.region}", prefix: "sessions" }
  EOT
}
