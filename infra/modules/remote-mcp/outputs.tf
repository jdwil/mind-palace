output "endpoint_url" {
  description = "MCP endpoint URL — point your agent client here"
  value       = "${local.public_url}/mcp"
}

output "alb_dns_name" {
  description = "ALB DNS name"
  value       = aws_lb.mcp.dns_name
}

output "health_url" {
  description = "Health check URL"
  value       = "${local.public_url}/health"
}

output "cluster_name" {
  description = "ECS cluster name"
  value       = aws_ecs_cluster.mcp.name
}

output "service_name" {
  description = "ECS service name"
  value       = aws_ecs_service.mcp.name
}

output "auth_enabled" {
  description = "Whether bearer-token auth is enabled"
  value       = local.use_auth
}

output "task_role_arn" {
  description = "Task role ARN (has Mind Palace runtime permissions)"
  value       = aws_iam_role.task.arn
}
