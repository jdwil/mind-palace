# Mind Palace Remote MCP — Terraform module (template)
#
# Deploys the mind-palace-mcp-remote server as a long-lived ECS Fargate service
# behind an (internal by default) Application Load Balancer.
#
# Copy this module into your project's Terraform, or reference it directly:
#
#   module "mind_palace_mcp" {
#     source = "github.com/jdwil/mind-palace//infra/modules/remote-mcp"
#
#     name        = "mind-palace-mcp"
#     region      = "us-east-1"
#     vpc_id      = "vpc-xxxx"
#     subnet_ids  = ["subnet-a", "subnet-b"]   # private subnets for the service
#     alb_subnet_ids = ["subnet-a", "subnet-b"] # subnets for the ALB (internal)
#
#     image = "<account>.dkr.ecr.us-east-1.amazonaws.com/mind-palace-mcp-remote:latest"
#
#     pages_bucket   = "mind-palace-pages-prod-xxxx"
#     vectors_bucket = "mind-palace-vectors-prod-xxxx"
#     graph_table    = "mind-palace-graph-prod"
#
#     # Auth: leave auth_token null for VPN/private (no auth), or set a token.
#     auth_token = null
#   }

variable "name" {
  description = "Name prefix for all resources"
  type        = string
  default     = "mind-palace-mcp"
}

variable "region" {
  description = "AWS region"
  type        = string
}

# --- Networking ---

variable "vpc_id" {
  description = "VPC to deploy into"
  type        = string
}

variable "subnet_ids" {
  description = "Subnet IDs for the ECS service (private recommended)"
  type        = list(string)
}

variable "alb_subnet_ids" {
  description = "Subnet IDs for the load balancer"
  type        = list(string)
}

variable "internal" {
  description = "If true (default), the ALB is internal (VPN/private access only). Set false for an internet-facing ALB (require auth_token in that case)."
  type        = bool
  default     = true
}

variable "ingress_cidrs" {
  description = "CIDR blocks allowed to reach the ALB (e.g. your VPC or VPN CIDR)."
  type        = list(string)
  default     = ["10.0.0.0/8"]
}

# --- Container ---

variable "image" {
  description = "ECR image URI for mind-palace-mcp-remote"
  type        = string
}

variable "cpu" {
  description = "Fargate task CPU units"
  type        = number
  default     = 512
}

variable "memory" {
  description = "Fargate task memory (MiB)"
  type        = number
  default     = 1024
}

variable "desired_count" {
  description = "Number of service tasks"
  type        = number
  default     = 1
}

variable "container_port" {
  description = "Port the server listens on"
  type        = number
  default     = 8080
}

# --- Mind Palace config ---

variable "pages_bucket" {
  description = "S3 pages bucket name"
  type        = string
}

variable "vectors_bucket" {
  description = "S3 Vectors bucket name"
  type        = string
}

variable "vectors_index" {
  description = "S3 Vectors index name"
  type        = string
  default     = "wiki-pages"
}

variable "graph_table" {
  description = "DynamoDB graph table name"
  type        = string
}

variable "s3_prefix" {
  description = "S3 page key prefix"
  type        = string
  default     = "v1"
}

variable "embedding_model" {
  description = "Bedrock embedding model ID"
  type        = string
  default     = "amazon.titan-embed-text-v2:0"
}

# --- Auth ---

variable "auth_token" {
  description = "Optional bearer token. null/empty = auth DISABLED (only use behind a VPN/internal ALB). If set, stored in Secrets Manager and injected as MP_AUTH_TOKEN."
  type        = string
  default     = null
  sensitive   = true
}

variable "allowed_hosts" {
  description = "Optional comma-separated Host allowlist (MP_ALLOWED_HOSTS). Empty = host validation disabled (needed behind an ALB)."
  type        = string
  default     = ""
}

variable "log_retention_days" {
  description = "CloudWatch log retention"
  type        = number
  default     = 14
}

variable "tags" {
  description = "Extra tags applied to all resources"
  type        = map(string)
  default     = {}
}
