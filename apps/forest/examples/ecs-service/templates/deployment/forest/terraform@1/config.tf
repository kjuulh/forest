terraform {
  required_providers {
    aws = {
      source  = "hashicorp/aws"
      version = "~> 5.0"
    }
  }
}

provider "aws" {
  region = "eu-west-1"
}

locals {
  full_config = jsondecode(file(var.config_file))
  env         = local.full_config.env
  config      = local.full_config.config

  name     = local.config.name
  port     = tonumber(local.config.port)
  replicas = tonumber(local.config.replicas)
  image    = local.config.image

  command           = try(local.config.command, [])
  cpu               = tostring(try(local.config.cpu, "256"))
  memory            = tostring(try(local.config.memory, "512"))
  env_vars          = try(local.config.env_vars, {})
  secret_keys       = try(local.config.secrets, [])
  health_check_path = try(local.config.health_check_path, "/")
  host_headers      = try(local.config.host_headers, [])
  priority          = tonumber(local.config.priority)

  common_tags = {
    Application   = local.name
    Environment   = local.env
    ManagedBy     = "forest"
    ForestProject = local.name
  }
}
