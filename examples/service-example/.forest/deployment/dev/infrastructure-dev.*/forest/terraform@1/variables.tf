variable "environment" {
  type        = string
  description = "Environment name to print"
}

variable "config_file" {
  type    = string
  default = "forest/config.json"
}
