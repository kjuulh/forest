locals {
  # Same structure as your Jinja `config` object
  full_config = jsondecode(file(var.config_file))
  config = try(local.full_config.config, {})
}
