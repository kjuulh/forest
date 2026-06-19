# =============================================================================
# ECS service + supporting resources for `${local.name}` on the shared
# platform ALB. This deliberately mirrors infrastructure-platform's
# `modules/ecs-task` shape so a forest-managed service is the same surface
# as a TF-managed one — just owned by a different state file.
# =============================================================================

# -----------------------------------------------------------------------------
# Per-service IAM role (the task role; distinct from the shared task
# execution role). Empty by default — apps that need AWS permissions add
# inline policies at the consumer level.
# -----------------------------------------------------------------------------
resource "aws_iam_role" "task" {
  name = "${local.name}-${local.env}-task"

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

  tags = local.common_tags
}

# -----------------------------------------------------------------------------
# CloudWatch logs.
# -----------------------------------------------------------------------------
resource "aws_cloudwatch_log_group" "this" {
  name              = "/ecs/${local.name}-${local.env}"
  retention_in_days = 14

  tags = local.common_tags
}

# -----------------------------------------------------------------------------
# Task definition.
# -----------------------------------------------------------------------------
locals {
  container_environment = [
    for k, v in local.env_vars : {
      name  = k
      value = v
    }
  ]

  container_secrets = length(local.secret_keys) > 0 ? [
    for key in local.secret_keys : {
      name      = key
      valueFrom = "${data.aws_secretsmanager_secret.service_env[0].arn}:${key}::"
    }
  ] : []

  container_definition = {
    name      = local.name
    image     = local.image
    essential = true

    command = length(local.command) > 0 ? local.command : null

    portMappings = [{
      containerPort = local.port
      protocol      = "tcp"
    }]

    environment = local.container_environment
    secrets     = local.container_secrets

    repositoryCredentials = {
      credentialsParameter = data.aws_secretsmanager_secret.ghcr_pull.arn
    }

    logConfiguration = {
      logDriver = "awslogs"
      options = {
        "awslogs-group"         = aws_cloudwatch_log_group.this.name
        "awslogs-region"        = data.aws_region.current.name
        "awslogs-stream-prefix" = "ecs"
      }
    }
  }
}

resource "aws_ecs_task_definition" "this" {
  family                   = "${local.name}-${local.env}"
  network_mode             = "awsvpc"
  requires_compatibilities = ["FARGATE"]
  cpu                      = local.cpu
  memory                   = local.memory
  execution_role_arn       = data.aws_iam_role.task_execution.arn
  task_role_arn            = aws_iam_role.task.arn

  container_definitions = jsonencode([local.container_definition])

  tags = local.common_tags
}

# -----------------------------------------------------------------------------
# ALB target group + listener rule on the shared HTTPS listener.
# -----------------------------------------------------------------------------
resource "aws_lb_target_group" "this" {
  # ALB target-group names are capped at 32 chars.
  name        = substr("fr-${local.name}-${local.env}", 0, 32)
  port        = local.port
  protocol    = "HTTP"
  target_type = "ip"
  vpc_id      = data.aws_vpc.platform.id

  health_check {
    path                = local.health_check_path
    matcher             = "200-399"
    interval            = 30
    timeout             = 5
    healthy_threshold   = 2
    unhealthy_threshold = 3
  }

  tags = local.common_tags
}

resource "aws_lb_listener_rule" "this" {
  listener_arn = data.aws_lb_listener.https.arn
  priority     = local.priority

  action {
    type             = "forward"
    target_group_arn = aws_lb_target_group.this.arn
  }

  # If host_headers is empty, fall through to a path match on `/*` so the
  # rule still has a valid condition (ALB requires at least one).
  dynamic "condition" {
    for_each = length(local.host_headers) > 0 ? [1] : []
    content {
      host_header {
        values = local.host_headers
      }
    }
  }

  dynamic "condition" {
    for_each = length(local.host_headers) == 0 ? [1] : []
    content {
      path_pattern {
        values = ["/*"]
      }
    }
  }

  tags = local.common_tags
}

# -----------------------------------------------------------------------------
# ECS service.
# -----------------------------------------------------------------------------
resource "aws_ecs_service" "this" {
  name            = "${local.name}-${local.env}"
  cluster         = data.aws_ecs_cluster.platform.id
  task_definition = aws_ecs_task_definition.this.arn
  desired_count   = local.replicas
  launch_type     = "FARGATE"

  network_configuration {
    subnets = data.aws_subnets.private.ids
    security_groups = [
      data.aws_security_group.ecs_tasks.id,
      data.aws_security_group.internal_access.id,
    ]
    assign_public_ip = false
  }

  load_balancer {
    target_group_arn = aws_lb_target_group.this.arn
    container_name   = local.name
    container_port   = local.port
  }

  # Don't fight an out-of-band scale change while we figure out HPA.
  lifecycle {
    ignore_changes = [desired_count]
  }

  depends_on = [aws_lb_listener_rule.this]

  tags = local.common_tags
}

# -----------------------------------------------------------------------------
# Outputs (useful for `forest release` log readout).
# -----------------------------------------------------------------------------
output "service_arn" {
  value = aws_ecs_service.this.id
}

output "task_definition_arn" {
  value = aws_ecs_task_definition.this.arn
}

output "target_group_arn" {
  value = aws_lb_target_group.this.arn
}
