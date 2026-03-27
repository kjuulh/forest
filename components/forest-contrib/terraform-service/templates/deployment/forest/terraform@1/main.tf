resource "null_resource" "hello" {
  provisioner "local-exec" {
    command = "echo Hello ${local.full_config.env}@${local.config.name} with ${local.config.replicas} replicas"
  }
}

resource "time_sleep" "wait" {
  depends_on      = [null_resource.hello]
  create_duration = "5s"
}
