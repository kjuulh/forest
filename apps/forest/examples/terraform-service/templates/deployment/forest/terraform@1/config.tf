terraform {
  required_providers {
    null = {
      source = "hashicorp/null"
    }
    time = {
      source = "hashicorp/time"
    }
  }
}

locals {
  full_config = jsondecode(file(var.config_file))
  config = try(local.full_config.config, {})
}
