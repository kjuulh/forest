terraform {
  required_providers {
    aws = {
      source  = "hashicorp/aws"
      version = "~> 5.0"
    }
  }
}

provider "aws" {
  region = var.aws_region
}

locals {
  # Same structure as your Jinja `config` object
  full_config = jsondecode(file(var.config_file))

  config = try(local.full_config.config, {})

  ports = try(local.config.ports, [])

  # first live http healthcheck (if defined)
  live_http = try(local.config.health_checks.live.http, null)

  container_base = {
    name      = local.config.name
    image     = coalesce(try(local.config.image, null), "${var.aws_account_id}.dkr.ecr.${var.aws_region}.amazonaws.com/${local.config.name}:${local.config.version}")
    essential = true

    portMappings = [
      for p in local.ports : {
        containerPort = p.port
        protocol      = lower(try(p.protocol, "tcp"))
      }
    ]

    environment = [
      for e in try(local.config.environment, []) : {
        name  = e.key
        value = e.value
      }
    ]
  }

  container_healthcheck = local.live_http == null ? {} : {
    healthCheck = {
      command     = ["CMD-SHELL", "curl -f http://localhost:${local.live_http.port}${local.live_http.path} || exit 1"]
      interval    = try(local.config.health_checks.live.period_seconds, 10)
      timeout     = try(local.config.health_checks.live.timeout_seconds, 5)
      retries     = try(local.config.health_checks.live.failure_threshold, 3)
      startPeriod = try(local.config.health_checks.live.initial_delay_seconds, 30)
    }
  }

  container = merge(local.container_base, local.container_healthcheck)

  # Any port marked external in config.ports
  external_ports = [
    for p in local.ports : p if try(p.external, false)
  ]
}

resource "aws_ecs_task_definition" "this" {
  family                   = local.config.name
  requires_compatibilities = ["FARGATE"]
  network_mode             = "awsvpc"

  # Use ECS-specific task sizing from config.resources.task
  cpu    = try(local.config.resources.task.cpu, "256")
  memory = try(local.config.resources.task.memory, "512")

  execution_role_arn = var.task_execution_role_arn
  task_role_arn      = var.task_role_arn

  container_definitions = jsonencode([local.container])
}

resource "aws_ecs_service" "this" {
  name            = local.config.name
  cluster         = var.ecs_cluster_arn
  launch_type     = "FARGATE"
  desired_count   = try(local.config.replicas, 1)
  task_definition = aws_ecs_task_definition.this.arn

  deployment_minimum_healthy_percent = 50
  deployment_maximum_percent         = 200

  network_configuration {
    subnets         = var.subnet_ids
    security_groups = var.security_group_ids

    # crude mapping of "external" -> public IP; in reality you'd usually front this with an ALB
    assign_public_ip = length(local.external_ports) > 0 ? "ENABLED" : "DISABLED"
  }

  # If you want ALB integration, you'd add a load_balancer {} block here and
  # use local.external_ports[*].port for target groups.
}
