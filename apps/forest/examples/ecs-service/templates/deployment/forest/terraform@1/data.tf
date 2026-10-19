data "aws_caller_identity" "current" {}

data "aws_region" "current" {}

data "aws_ecs_cluster" "platform" {
  cluster_name = "infrastructure-platform"
}

data "aws_lb" "shared" {
  name = "shared-alb"
}

data "aws_lb_listener" "https" {
  load_balancer_arn = data.aws_lb.shared.arn
  port              = 443
}

data "aws_vpc" "platform" {
  filter {
    name   = "tag:Name"
    values = ["platform"]
  }
}

data "aws_subnets" "private" {
  filter {
    name   = "vpc-id"
    values = [data.aws_vpc.platform.id]
  }
  filter {
    name   = "tag:Tier"
    values = ["private"]
  }
}

data "aws_security_group" "ecs_tasks" {
  name = "ecs-tasks"
}

data "aws_security_group" "internal_access" {
  name = "internal-access"
}

data "aws_secretsmanager_secret" "ghcr_pull" {
  name = "${local.env}/ghcr/pull-secret"
}

data "aws_secretsmanager_secret" "service_env" {
  count = length(local.secret_keys) > 0 ? 1 : 0
  name  = "${local.env}/${local.name}/env"
}

data "aws_iam_role" "task_execution" {
  name = "ecsTaskExecutionRole"
}
