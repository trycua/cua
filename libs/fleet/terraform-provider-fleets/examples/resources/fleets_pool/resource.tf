resource "fleets_pool" "example" {
  name                 = "example-linux"
  replicas             = 1
  cpu_cores            = 4
  memory               = "8Gi"
  container_disk_image = "public.ecr.aws/k5j5w0x5/cua-ubuntu-24.04:main-e5d853a9"
}
