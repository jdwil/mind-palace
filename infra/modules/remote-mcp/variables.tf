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

variable "auth_mode" {
  description = "Auth mode: 'none' (anonymous, VPN only — sees only public pages), 'token' (shared bearer), or 'oidc' (per-user JWT identity)."
  type        = string
  default     = "none"
  validation {
    condition     = contains(["none", "token", "oidc"], var.auth_mode)
    error_message = "auth_mode must be one of: none, token, oidc."
  }
}

variable "auth_token" {
  description = "Shared bearer token. Required when auth_mode = 'token'. Stored in Secrets Manager and injected as MP_AUTH_TOKEN."
  type        = string
  default     = null
  sensitive   = true
}

# --- OIDC (used when auth_mode = 'oidc') ---

variable "oidc_issuer" {
  description = "OIDC issuer URL. For Cognito: https://cognito-idp.<region>.amazonaws.com/<userPoolId>. Required when auth_mode = 'oidc'."
  type        = string
  default     = ""
}

variable "oidc_jwks_url" {
  description = "JWKS endpoint. Defaults (in the server) to <issuer>/.well-known/jwks.json — leave empty for Cognito."
  type        = string
  default     = ""
}

variable "oidc_audiences" {
  description = "Comma-separated allowed aud/client_id values (your app client IDs). Strongly recommended in production."
  type        = string
  default     = ""
}

variable "oidc_email_claim" {
  description = "JWT claim used as the user identity."
  type        = string
  default     = "email"
}

variable "oidc_fallback_claim" {
  description = "Identity claim used when the email claim is absent (M2M tokens)."
  type        = string
  default     = "sub"
}

variable "public_url" {
  description = "The server's externally reachable URL (e.g. https://mind-palace.example.com). Advertised in OIDC discovery metadata. Required for oidc mode; if empty, defaults to the ALB DNS (https when a cert is set, else http)."
  type        = string
  default     = ""
}

# --- HTTPS ---

variable "acm_certificate_arn" {
  description = "ACM certificate ARN for an HTTPS (443) listener. REQUIRED for auth_mode 'token' or 'oidc' (bearer credentials must not travel plaintext). If empty, only an HTTP (80) listener is created — acceptable ONLY for auth_mode 'none' behind a VPN."
  type        = string
  default     = ""
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
