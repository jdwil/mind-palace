# Mind Palace — Dreaming Process
#
# Runs the mind-palace-dream binary on a daily schedule.
# Reads session logs from S3, analyzes via Bedrock Claude, updates wiki.

locals {
  dream_name = "mind-palace-dream"
}

# =============================================================================
# ECR Repository
# =============================================================================

resource "aws_ecr_repository" "dream" {
  name                 = local.dream_name
  image_tag_mutability = "MUTABLE"
  force_delete         = true

  image_scanning_configuration {
    scan_on_push = true
  }

  tags = {
    Component = "dreaming"
  }
}

resource "aws_ecr_lifecycle_policy" "dream" {
  repository = aws_ecr_repository.dream.name

  policy = jsonencode({
    rules = [{
      rulePriority = 1
      description  = "Keep last 5 images"
      selection = {
        tagStatus   = "any"
        countType   = "imageCountMoreThan"
        countNumber = 5
      }
      action = {
        type = "expire"
      }
    }]
  })
}

# =============================================================================
# ECS Cluster (or use existing)
# =============================================================================

resource "aws_ecs_cluster" "dream" {
  count = var.ecs_cluster_arn == "" ? 1 : 0
  name  = local.dream_name

  setting {
    name  = "containerInsights"
    value = "enabled"
  }

  tags = {
    Component = "dreaming"
  }
}

locals {
  cluster_arn = var.ecs_cluster_arn != "" ? var.ecs_cluster_arn : aws_ecs_cluster.dream[0].arn
}

# =============================================================================
# IAM — Task Execution Role (for ECS to pull image, write logs)
# =============================================================================

resource "aws_iam_role" "dream_execution" {
  name = "${local.dream_name}-execution"

  assume_role_policy = jsonencode({
    Version = "2012-10-17"
    Statement = [{
      Effect = "Allow"
      Principal = {
        Service = "ecs-tasks.amazonaws.com"
      }
      Action = "sts:AssumeRole"
    }]
  })

  tags = {
    Component = "dreaming"
  }
}

resource "aws_iam_role_policy_attachment" "dream_execution" {
  role       = aws_iam_role.dream_execution.name
  policy_arn = "arn:aws:iam::aws:policy/service-role/AmazonECSTaskExecutionRolePolicy"
}

# =============================================================================
# IAM — Task Role (what the container can do)
# =============================================================================

resource "aws_iam_role" "dream_task" {
  name = "${local.dream_name}-task"

  assume_role_policy = jsonencode({
    Version = "2012-10-17"
    Statement = [{
      Effect = "Allow"
      Principal = {
        Service = "ecs-tasks.amazonaws.com"
      }
      Action = "sts:AssumeRole"
    }]
  })

  tags = {
    Component = "dreaming"
  }
}

# Attach the Mind Palace runtime policy (defined in main.tf)
resource "aws_iam_role_policy_attachment" "dream_mind_palace" {
  role       = aws_iam_role.dream_task.name
  policy_arn = aws_iam_policy.mind_palace.arn
}

# Additional: Bedrock Claude access for the LLM calls
resource "aws_iam_role_policy" "dream_bedrock_llm" {
  name = "bedrock-llm"
  role = aws_iam_role.dream_task.id

  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [{
      Sid    = "BedrockLLM"
      Effect = "Allow"
      Action = [
        "bedrock:InvokeModel",
        "bedrock:InvokeModelWithResponseStream"
      ]
      Resource = [
        "arn:aws:bedrock:${var.region}::foundation-model/${var.llm_model_id}",
        "arn:aws:bedrock:${var.region}::foundation-model/anthropic.claude-haiku-4-20250514-v1:0"
      ]
    }]
  })
}

# =============================================================================
# CloudWatch Log Group
# =============================================================================

resource "aws_cloudwatch_log_group" "dream" {
  name              = "/ecs/${local.dream_name}"
  retention_in_days = 14

  tags = {
    Component = "dreaming"
  }
}

# =============================================================================
# ECS Task Definition
# =============================================================================

resource "aws_ecs_task_definition" "dream" {
  family                   = local.dream_name
  requires_compatibilities = ["FARGATE"]
  network_mode             = "awsvpc"
  cpu                      = 512
  memory                   = 1024
  execution_role_arn       = aws_iam_role.dream_execution.arn
  task_role_arn            = aws_iam_role.dream_task.arn

  container_definitions = jsonencode([{
    name  = local.dream_name
    image = "${aws_ecr_repository.dream.repository_url}:latest"

    essential = true

    environment = [
      { name = "MIND_PALACE_S3_BUCKET", value = aws_s3_bucket.pages.id },
      { name = "MIND_PALACE_S3_PREFIX", value = "v1" },
      { name = "MIND_PALACE_DYNAMO_TABLE", value = aws_dynamodb_table.graph.name },
      { name = "MIND_PALACE_VECTORS_BUCKET", value = local.vectors_bucket },
      { name = "MIND_PALACE_VECTORS_INDEX", value = var.vectors_index_name },
      { name = "MIND_PALACE_BEDROCK_MODEL", value = "amazon.titan-embed-text-v2:0" },
      { name = "MIND_PALACE_REGION", value = var.region },
      { name = "MP_LOG_BUCKET", value = aws_s3_bucket.logs.id },
      { name = "MP_LOG_PREFIX", value = "sessions" },
      { name = "MP_LLM_MODEL_ID", value = var.llm_model_id },
      { name = "RUST_LOG", value = "mind_palace_dream=info" },
    ]

    logConfiguration = {
      logDriver = "awslogs"
      options = {
        "awslogs-group"         = aws_cloudwatch_log_group.dream.name
        "awslogs-region"        = var.region
        "awslogs-stream-prefix" = "dream"
      }
    }
  }])

  tags = {
    Component = "dreaming"
  }
}

# =============================================================================
# EventBridge Schedule (daily at 3am UTC)
# =============================================================================

resource "aws_iam_role" "dream_scheduler" {
  name = "${local.dream_name}-scheduler"

  assume_role_policy = jsonencode({
    Version = "2012-10-17"
    Statement = [{
      Effect = "Allow"
      Principal = {
        Service = "scheduler.amazonaws.com"
      }
      Action = "sts:AssumeRole"
    }]
  })

  tags = {
    Component = "dreaming"
  }
}

resource "aws_iam_role_policy" "dream_scheduler" {
  name = "run-task"
  role = aws_iam_role.dream_scheduler.id

  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [
      {
        Effect   = "Allow"
        Action   = "ecs:RunTask"
        Resource = aws_ecs_task_definition.dream.arn
        Condition = {
          ArnEquals = {
            "ecs:cluster" = local.cluster_arn
          }
        }
      },
      {
        Effect = "Allow"
        Action = "iam:PassRole"
        Resource = [
          aws_iam_role.dream_execution.arn,
          aws_iam_role.dream_task.arn
        ]
      }
    ]
  })
}

resource "aws_scheduler_schedule" "dream" {
  name       = local.dream_name
  group_name = "default"

  schedule_expression          = var.schedule_expression
  schedule_expression_timezone = "UTC"

  flexible_time_window {
    mode                      = "FLEXIBLE"
    maximum_window_in_minutes = 30
  }

  target {
    arn      = local.cluster_arn
    role_arn = aws_iam_role.dream_scheduler.arn

    ecs_parameters {
      task_definition_arn = aws_ecs_task_definition.dream.arn
      launch_type         = "FARGATE"
      platform_version    = "LATEST"

      network_configuration {
        subnets          = var.subnet_ids
        security_groups  = var.security_group_ids
        assign_public_ip = false
      }
    }
  }

  state = var.schedule_enabled ? "ENABLED" : "DISABLED"
}
