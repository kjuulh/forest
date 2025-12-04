resource "null_resource" "hello" {
  provisioner "local-exec" {
    command = "echo Hello ${var.environment}@${local.config.name}"
  }
}
