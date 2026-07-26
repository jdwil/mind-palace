variable "region" {
  description = "AWS region"
  type        = string
  default     = "us-west-2"
}

variable "environment" {
  description = "Deployment environment (dev, staging, prod)"
  type        = string
  default     = "dev"

  validation {
    condition     = contains(["dev", "staging", "prod"], var.environment)
    error_message = "Environment must be one of: dev, staging, prod."
  }
}

variable "embedding_dimensions" {
  description = "Embedding vector dimensions (1024 for Titan Embeddings v2)"
  type        = number
  default     = 1024
}

variable "vectors_index_name" {
  description = "S3 Vectors index name"
  type        = string
  default     = "wiki-pages"
}

variable "llm_model_id" {
  description = "Bedrock model ID for the dreaming LLM"
  type        = string
  default     = "anthropic.claude-sonnet-4-20250514-v1:0"
}

variable "ecs_cluster_arn" {
  description = "Existing ECS cluster ARN. If empty, a new cluster is created."
  type        = string
  default     = ""
}

variable "subnet_ids" {
  description = "Private subnet IDs for the ECS task (need internet via NAT for Bedrock)"
  type        = list(string)
}

variable "security_group_ids" {
  description = "Security group IDs for the ECS task"
  type        = list(string)
}

variable "schedule_expression" {
  description = "EventBridge schedule expression (cron or rate)"
  type        = string
  default     = "cron(0 3 * * ? *)"
}

variable "schedule_enabled" {
  description = "Whether the dreaming schedule is enabled"
  type        = bool
  default     = true
}
