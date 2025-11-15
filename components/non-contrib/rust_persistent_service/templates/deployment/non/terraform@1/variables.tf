variable "aws_region" {
  type = string
}

variable "aws_account_id" {
  type = string
}

variable "ecs_cluster_arn" {
  type = string
}

variable "subnet_ids" {
  type = list(string)
}

variable "security_group_ids" {
  type = list(string)
}

variable "task_execution_role_arn" {
  type = string
}

variable "task_role_arn" {
  type = string
}

variable "config_file" {
  type    = string
  default = "non/config.json"
}
