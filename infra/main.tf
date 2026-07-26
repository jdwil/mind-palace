# Mind Palace — Core Infrastructure
#
# S3 buckets, DynamoDB table, S3 Vectors, IAM policy.

locals {
  account = data.aws_caller_identity.current.account_id
  name    = "mind-palace"

  pages_bucket   = "${local.name}-pages-${var.environment}-${local.account}"
  vectors_bucket = "${local.name}-vectors-${var.environment}-${local.account}"
  logs_bucket    = "${local.name}-logs-${var.environment}-${local.account}"
  graph_table    = "${local.name}-graph-${var.environment}"
}

data "aws_caller_identity" "current" {}

# =============================================================================
# S3 — Pages Bucket (wiki content, versioned)
# =============================================================================

resource "aws_s3_bucket" "pages" {
  bucket = local.pages_bucket

  tags = {
    Component = "pages"
  }
}

resource "aws_s3_bucket_versioning" "pages" {
  bucket = aws_s3_bucket.pages.id

  versioning_configuration {
    status = "Enabled"
  }
}

resource "aws_s3_bucket_server_side_encryption_configuration" "pages" {
  bucket = aws_s3_bucket.pages.id

  rule {
    apply_server_side_encryption_by_default {
      sse_algorithm = "AES256"
    }
  }
}

resource "aws_s3_bucket_public_access_block" "pages" {
  bucket = aws_s3_bucket.pages.id

  block_public_acls       = true
  block_public_policy     = true
  ignore_public_acls      = true
  restrict_public_buckets = true
}

# =============================================================================
# DynamoDB — Graph Table (page metadata + relationships)
# =============================================================================

resource "aws_dynamodb_table" "graph" {
  name         = local.graph_table
  billing_mode = "PAY_PER_REQUEST"
  hash_key     = "PK"
  range_key    = "SK"

  attribute {
    name = "PK"
    type = "S"
  }

  attribute {
    name = "SK"
    type = "S"
  }

  point_in_time_recovery {
    enabled = true
  }

  tags = {
    Component = "graph"
  }
}

# =============================================================================
# S3 Vectors — Semantic Search
#
# NOTE: S3 Vectors (AWS::S3::VectorBucket / AWS::S3::VectorIndex) do not yet
# have Terraform provider support. Create the vector bucket and index manually:
#
#   aws s3vectors create-vector-bucket --vector-bucket-name <vectors_bucket_name>
#   aws s3vectors create-index \
#     --vector-bucket-name <vectors_bucket_name> \
#     --index-name wiki-pages \
#     --dimensions 1024 \
#     --distance-metric cosine
#
# The bucket name follows the convention: mind-palace-vectors-<env>-<account_id>
# =============================================================================

# Placeholder bucket — use the AWS CLI commands above for the actual vector bucket.
# This regular S3 bucket is NOT the vector bucket; it exists only as a Terraform
# reference point so outputs and IAM policies have a consistent name.

# =============================================================================
# S3 — Logs Bucket (agent session logs, 30-day lifecycle)
# =============================================================================

resource "aws_s3_bucket" "logs" {
  bucket = local.logs_bucket

  tags = {
    Component = "logs"
  }
}

resource "aws_s3_bucket_lifecycle_configuration" "logs" {
  bucket = aws_s3_bucket.logs.id

  rule {
    id     = "expire-processed-logs"
    status = "Enabled"

    filter {}

    expiration {
      days = 30
    }
  }
}

resource "aws_s3_bucket_server_side_encryption_configuration" "logs" {
  bucket = aws_s3_bucket.logs.id

  rule {
    apply_server_side_encryption_by_default {
      sse_algorithm = "AES256"
    }
  }
}

resource "aws_s3_bucket_public_access_block" "logs" {
  bucket = aws_s3_bucket.logs.id

  block_public_acls       = true
  block_public_policy     = true
  ignore_public_acls      = true
  restrict_public_buckets = true
}

# =============================================================================
# IAM — Mind Palace Runtime Policy
# =============================================================================

resource "aws_iam_policy" "mind_palace" {
  name        = "${local.name}-runtime-${var.environment}"
  description = "Permissions for Mind Palace runtime access"

  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [
      {
        Sid    = "S3Pages"
        Effect = "Allow"
        Action = [
          "s3:GetObject",
          "s3:PutObject",
          "s3:DeleteObject",
          "s3:ListBucket",
          "s3:GetObjectVersion"
        ]
        Resource = [
          aws_s3_bucket.pages.arn,
          "${aws_s3_bucket.pages.arn}/*"
        ]
      },
      {
        Sid    = "DynamoDB"
        Effect = "Allow"
        Action = [
          "dynamodb:Scan",
          "dynamodb:Query",
          "dynamodb:PutItem",
          "dynamodb:DeleteItem",
          "dynamodb:BatchWriteItem",
          "dynamodb:GetItem"
        ]
        Resource = [
          aws_dynamodb_table.graph.arn
        ]
      },
      {
        Sid    = "S3Vectors"
        Effect = "Allow"
        Action = [
          "s3vectors:PutVectors",
          "s3vectors:QueryVectors",
          "s3vectors:DeleteVectors",
          "s3vectors:GetVectors"
        ]
        Resource = "*"
      },
      {
        Sid    = "S3Logs"
        Effect = "Allow"
        Action = [
          "s3:GetObject",
          "s3:PutObject",
          "s3:DeleteObject",
          "s3:ListBucket"
        ]
        Resource = [
          aws_s3_bucket.logs.arn,
          "${aws_s3_bucket.logs.arn}/*"
        ]
      },
      {
        Sid    = "Bedrock"
        Effect = "Allow"
        Action = [
          "bedrock:InvokeModel"
        ]
        Resource = [
          "arn:aws:bedrock:${var.region}::foundation-model/amazon.titan-embed-text-v2:0"
        ]
      }
    ]
  })

  tags = {
    Component = "iam"
  }
}
