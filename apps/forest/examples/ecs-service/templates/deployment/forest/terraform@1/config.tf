terraform {
  required_providers {
    aws = {
      source  = "hashicorp/aws"
      version = "~> 5.0"
    }
  }
}

# AWS region — the platform-dev account lives in eu-west-1. The terraform v1
# destination doesn't currently expose target-region as an input, so we pin
# it here and rely on the runner's AWS credentials being scoped to the same
# account.
provider "aws" {
  region = "eu-west-1"
}

locals {
  # `full_config` carries the runtime context forest hands to terraform:
  #   {
  #     "env":    "dev" | "staging" | "prod",
  #     "config": <#Spec from forest.cue>
  #   }
  full_config = jsondecode(file(var.config_file))
  env         = local.full_config.env
  config      = local.full_config.config

  # Convenience handles.
  name     = local.config.name
  port     = tonumber(local.config.port)
  replicas = tonumber(local.config.replicas)
  image    = local.config.image

  # Optional spec fields with defaults. CUE applies the defaults when the
  # consumer omits them, but the keys are always present in the JSON.
  command           = try(local.config.command, [])
  cpu               = tostring(try(local.config.cpu, "256"))
  memory            = tostring(try(local.config.memory, "512"))
  env_vars          = try(local.config.env_vars, {})
  secret_keys       = try(local.config.secrets, [])
  health_check_path = try(local.config.health_check_path, "/")
  host_headers      = try(local.config.host_headers, [])
  priority          = tonumber(local.config.priority)

  # Tags applied to everything we create, so platform infra can find these.
  common_tags = {
    Application   = local.name
    Environment   = local.env
    ManagedBy     = "forest"
    ForestProject = local.name
  }
}
