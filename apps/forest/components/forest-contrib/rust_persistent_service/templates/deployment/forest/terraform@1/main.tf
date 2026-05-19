resource "null_resource" "hello" {
  provisioner "local-exec" {
    command = "echo Hello ${var.environment}@${local.config.name}"
  }
}

resource "time_sleep" "wait" {
  depends_on      = [null_resource.hello]
  create_duration = "5s"
}
