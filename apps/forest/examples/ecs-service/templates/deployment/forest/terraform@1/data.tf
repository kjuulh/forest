# =============================================================================
# Look up shared platform infrastructure by name. These are all created and
# managed by infrastructure-platform and discovered here at apply time so we
# don't have to thread ARNs through forest config.
# =============================================================================

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

# `ecs-tasks` is the egress-only SG every Fargate task in this account
# uses. `internal-access` allows ingress from the VPC CIDR so the ALB can
# reach the task. Both are created in infrastructure-platform/ecs.tf and
# vpc/security-groups.tf.
data "aws_security_group" "ecs_tasks" {
  name = "ecs-tasks"
}

data "aws_security_group" "internal_access" {
  name = "internal-access"
}

# GHCR pull secret for the task execution role. Created in
# infrastructure-platform/ecs.tf as `${env}/ghcr/pull-secret`.
data "aws_secretsmanager_secret" "ghcr_pull" {
  name = "${local.env}/ghcr/pull-secret"
}

# Per-service env+secrets bundle. Created out-of-band by the operator (or a
# follow-up component) under `${env}/${name}/env`. Forage and forest both
# follow this convention.
data "aws_secretsmanager_secret" "service_env" {
  count = length(local.secret_keys) > 0 ? 1 : 0
  name  = "${local.env}/${local.name}/env"
}

# Reuse the platform ECS task execution role rather than minting a per-service
# one. It already has logs + ghcr-pull + secret access scoped to
# `arn:aws:secretsmanager:eu-west-1:<account>:secret:*`.
data "aws_iam_role" "task_execution" {
  name = "ecsTaskExecutionRole"
}
