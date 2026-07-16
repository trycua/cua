resource "cyclops_pool" "example" {
  name                 = "example-linux"
  replicas             = 1
  cpu_cores            = 4
  memory               = "8Gi"
  container_disk_image = "296062593712.dkr.ecr.us-west-2.amazonaws.com/osgym-workspace:latest"
}
