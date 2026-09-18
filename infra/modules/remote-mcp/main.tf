# Mind Palace Remote MCP — ECS Fargate service behind an ALB.

locals {
  tags = merge({
    Project   = "mind-palace"
    Component = "remote-mcp"
    ManagedBy = "terraform"
  }, var.tags)

  # Token secret is created only in token mode with a token provided.
  use_auth = var.auth_mode == "token" && var.auth_token != null && var.auth_token != ""

  # HTTPS listener when a cert is supplied.
  use_https = var.acm_certificate_arn != ""

  # Advertised public URL: explicit override, else derive from ALB DNS + scheme.
  scheme      = local.use_https ? "https" : "http"
  derived_url = "${local.scheme}://${aws_lb.mcp.dns_name}"
  public_url  = var.public_url != "" ? var.public_url : local.derived_url
}

data "aws_caller_identity" "current" {}

# =============================================================================
# Optional: store the auth token in Secrets Manager
# =============================================================================

resource "aws_secretsmanager_secret" "auth" {
  count = local.use_auth ? 1 : 0
  name  = "${var.name}-auth-token"
  tags  = local.tags
}

resource "aws_secretsmanager_secret_version" "auth" {
  count         = local.use_auth ? 1 : 0
  secret_id     = aws_secretsmanager_secret.auth[0].id
  secret_string = var.auth_token
}

# =============================================================================
# CloudWatch Logs
# =============================================================================

resource "aws_cloudwatch_log_group" "mcp" {
  name              = "/ecs/${var.name}"
  retention_in_days = var.log_retention_days
  tags              = local.tags
}

# =============================================================================
# Security groups
# =============================================================================

resource "aws_security_group" "alb" {
  name        = "${var.name}-alb"
  description = "Mind Palace remote MCP ALB"
  vpc_id      = var.vpc_id

  ingress {
    description = "MCP HTTP in"
    from_port   = 80
    to_port     = 80
    protocol    = "tcp"
    cidr_blocks = var.ingress_cidrs
  }

  dynamic "ingress" {
    for_each = local.use_https ? [1] : []
    content {
      description = "MCP HTTPS in"
      from_port   = 443
      to_port     = 443
      protocol    = "tcp"
      cidr_blocks = var.ingress_cidrs
    }
  }

  egress {
    from_port   = 0
    to_port     = 0
    protocol    = "-1"
    cidr_blocks = ["0.0.0.0/0"]
  }

  tags = local.tags
}

resource "aws_security_group" "service" {
  name        = "${var.name}-service"
  description = "Mind Palace remote MCP ECS service"
  vpc_id      = var.vpc_id

  ingress {
    description     = "From ALB"
    from_port       = var.container_port
    to_port         = var.container_port
    protocol        = "tcp"
    security_groups = [aws_security_group.alb.id]
  }

  egress {
    from_port   = 0
    to_port     = 0
    protocol    = "-1"
    cidr_blocks = ["0.0.0.0/0"]
  }

  tags = local.tags
}

# =============================================================================
# Load balancer
# =============================================================================

resource "aws_lb" "mcp" {
  name               = var.name
  internal           = var.internal
  load_balancer_type = "application"
  subnets            = var.alb_subnet_ids
  security_groups    = [aws_security_group.alb.id]
  tags               = local.tags
}

resource "aws_lb_target_group" "mcp" {
  name        = var.name
  port        = var.container_port
  protocol    = "HTTP"
  vpc_id      = var.vpc_id
  target_type = "ip"

  health_check {
    path                = "/health"
    healthy_threshold   = 2
    unhealthy_threshold = 3
    interval            = 30
    timeout             = 5
    matcher             = "200"
  }

  tags = local.tags
}

resource "aws_lb_listener" "http" {
  load_balancer_arn = aws_lb.mcp.arn
  port              = 80
  protocol          = "HTTP"

  # When HTTPS is enabled, HTTP redirects to it; otherwise HTTP forwards
  # directly (acceptable only for auth_mode=none behind a VPN).
  dynamic "default_action" {
    for_each = local.use_https ? [1] : []
    content {
      type = "redirect"
      redirect {
        port        = "443"
        protocol    = "HTTPS"
        status_code = "HTTP_301"
      }
    }
  }

  dynamic "default_action" {
    for_each = local.use_https ? [] : [1]
    content {
      type             = "forward"
      target_group_arn = aws_lb_target_group.mcp.arn
    }
  }
}

resource "aws_lb_listener" "https" {
  count             = local.use_https ? 1 : 0
  load_balancer_arn = aws_lb.mcp.arn
  port              = 443
  protocol          = "HTTPS"
  ssl_policy        = "ELBSecurityPolicy-TLS13-1-2-2021-06"
  certificate_arn   = var.acm_certificate_arn

  default_action {
    type             = "forward"
    target_group_arn = aws_lb_target_group.mcp.arn
  }
}

# =============================================================================
# Configuration guardrails (fail plan on unsafe combos)
# =============================================================================

check "auth_requires_https" {
  assert {
    condition     = var.auth_mode == "none" || local.use_https
    error_message = "auth_mode '${var.auth_mode}' requires HTTPS: set acm_certificate_arn. Bearer tokens/JWTs must not travel plaintext."
  }
}

check "token_mode_requires_token" {
  assert {
    condition     = var.auth_mode != "token" || (var.auth_token != null && var.auth_token != "")
    error_message = "auth_mode 'token' requires auth_token to be set."
  }
}

check "oidc_mode_requires_issuer" {
  assert {
    condition     = var.auth_mode != "oidc" || var.oidc_issuer != ""
    error_message = "auth_mode 'oidc' requires oidc_issuer (e.g. https://cognito-idp.<region>.amazonaws.com/<userPoolId>)."
  }
}

# =============================================================================
# IAM
# =============================================================================

resource "aws_iam_role" "execution" {
  name = "${var.name}-execution"
  assume_role_policy = jsonencode({
    Version = "2012-10-17"
    Statement = [{
      Effect    = "Allow"
      Principal = { Service = "ecs-tasks.amazonaws.com" }
      Action    = "sts:AssumeRole"
    }]
  })
  tags = local.tags
}

resource "aws_iam_role_policy_attachment" "execution" {
  role       = aws_iam_role.execution.name
  policy_arn = "arn:aws:iam::aws:policy/service-role/AmazonECSTaskExecutionRolePolicy"
}

# Allow the execution role to read the auth secret (for injection).
resource "aws_iam_role_policy" "execution_secret" {
  count = local.use_auth ? 1 : 0
  name  = "read-auth-secret"
  role  = aws_iam_role.execution.id
  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [{
      Effect   = "Allow"
      Action   = ["secretsmanager:GetSecretValue"]
      Resource = [aws_secretsmanager_secret.auth[0].arn]
    }]
  })
}

resource "aws_iam_role" "task" {
  name = "${var.name}-task"
  assume_role_policy = jsonencode({
    Version = "2012-10-17"
    Statement = [{
      Effect    = "Allow"
      Principal = { Service = "ecs-tasks.amazonaws.com" }
      Action    = "sts:AssumeRole"
    }]
  })
  tags = local.tags
}

# Mind Palace runtime permissions (S3 pages, DynamoDB, S3 Vectors, Bedrock embed).
resource "aws_iam_role_policy" "task_runtime" {
  name = "mind-palace-runtime"
  role = aws_iam_role.task.id
  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [
      {
        Sid    = "S3Pages"
        Effect = "Allow"
        Action = [
          "s3:GetObject", "s3:PutObject", "s3:DeleteObject",
          "s3:ListBucket", "s3:GetObjectVersion"
        ]
        Resource = [
          "arn:aws:s3:::${var.pages_bucket}",
          "arn:aws:s3:::${var.pages_bucket}/*"
        ]
      },
      {
        Sid    = "DynamoDB"
        Effect = "Allow"
        Action = [
          "dynamodb:Scan", "dynamodb:Query", "dynamodb:GetItem",
          "dynamodb:PutItem", "dynamodb:DeleteItem", "dynamodb:BatchWriteItem"
        ]
        Resource = ["arn:aws:dynamodb:${var.region}:${data.aws_caller_identity.current.account_id}:table/${var.graph_table}"]
      },
      {
        Sid      = "S3Vectors"
        Effect   = "Allow"
        Action   = ["s3vectors:PutVectors", "s3vectors:QueryVectors", "s3vectors:DeleteVectors", "s3vectors:GetVectors"]
        Resource = "*"
      },
      {
        Sid      = "Bedrock"
        Effect   = "Allow"
        Action   = ["bedrock:InvokeModel"]
        Resource = ["arn:aws:bedrock:${var.region}::foundation-model/${var.embedding_model}"]
      }
    ]
  })
}

# =============================================================================
# ECS cluster + service
# =============================================================================

resource "aws_ecs_cluster" "mcp" {
  name = var.name
  tags = local.tags
}

resource "aws_ecs_task_definition" "mcp" {
  family                   = var.name
  requires_compatibilities = ["FARGATE"]
  network_mode             = "awsvpc"
  cpu                      = var.cpu
  memory                   = var.memory
  execution_role_arn       = aws_iam_role.execution.arn
  task_role_arn            = aws_iam_role.task.arn

  container_definitions = jsonencode([{
    name      = var.name
    image     = var.image
    essential = true

    portMappings = [{ containerPort = var.container_port, protocol = "tcp" }]

    environment = concat([
      { name = "MP_BIND_ADDR", value = "0.0.0.0:${var.container_port}" },
      { name = "MP_MCP_PATH", value = "/mcp" },
      { name = "MP_ALLOWED_HOSTS", value = var.allowed_hosts },
      { name = "MP_PUBLIC_URL", value = local.public_url },
      { name = "MP_AUTH_MODE", value = var.auth_mode },
      { name = "MIND_PALACE_S3_BUCKET", value = var.pages_bucket },
      { name = "MIND_PALACE_S3_PREFIX", value = var.s3_prefix },
      { name = "MIND_PALACE_DYNAMO_TABLE", value = var.graph_table },
      { name = "MIND_PALACE_VECTORS_BUCKET", value = var.vectors_bucket },
      { name = "MIND_PALACE_VECTORS_INDEX", value = var.vectors_index },
      { name = "MIND_PALACE_BEDROCK_MODEL", value = var.embedding_model },
      { name = "MIND_PALACE_REGION", value = var.region },
      { name = "RUST_LOG", value = "mind_palace_mcp=info,info" },
      ],
      var.auth_mode == "oidc" ? [
        { name = "MP_OIDC_ISSUER", value = var.oidc_issuer },
        { name = "MP_OIDC_JWKS_URL", value = var.oidc_jwks_url },
        { name = "MP_OIDC_AUDIENCES", value = var.oidc_audiences },
        { name = "MP_OIDC_EMAIL_CLAIM", value = var.oidc_email_claim },
        { name = "MP_OIDC_FALLBACK_CLAIM", value = var.oidc_fallback_claim },
      ] : []
    )

    secrets = local.use_auth ? [
      { name = "MP_AUTH_TOKEN", valueFrom = aws_secretsmanager_secret.auth[0].arn }
    ] : []

    logConfiguration = {
      logDriver = "awslogs"
      options = {
        "awslogs-group"         = aws_cloudwatch_log_group.mcp.name
        "awslogs-region"        = var.region
        "awslogs-stream-prefix" = "mcp"
      }
    }
  }])

  tags = local.tags
}

resource "aws_ecs_service" "mcp" {
  name            = var.name
  cluster         = aws_ecs_cluster.mcp.id
  task_definition = aws_ecs_task_definition.mcp.arn
  desired_count   = var.desired_count
  launch_type     = "FARGATE"

  network_configuration {
    subnets          = var.subnet_ids
    security_groups  = [aws_security_group.service.id]
    assign_public_ip = false
  }

  load_balancer {
    target_group_arn = aws_lb_target_group.mcp.arn
    container_name   = var.name
    container_port   = var.container_port
  }

  depends_on = [aws_lb_listener.http]

  tags = local.tags
}
