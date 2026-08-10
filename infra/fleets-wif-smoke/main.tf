terraform {
  backend "s3" {
    bucket         = "cloud-terraform-state-bc8146107c2d77da"
    key            = "cua/fleets-wif-smoke/terraform.tfstate"
    region         = "us-west-2"
    dynamodb_table = "cloud-terraform-lock-bc8146107c2d77da"
    encrypt        = true
  }

  required_providers {
    fleets = {
      source  = "trycua/fleets"
      version = "1.0.0"
    }
  }
}

provider "fleets" {
  endpoint = "https://run.cua.ai"
}

resource "fleets_pool" "cua_cli_wif_smoke" {
  name                 = "cua-cli-wif-smoke"
  replicas             = 0
  cpu_cores            = 4
  memory               = "8Gi"
  container_disk_image = "296062593712.dkr.ecr.us-west-2.amazonaws.com/desktop-workspace:latest"
  readiness_probe_json = jsonencode({ tcpSocket = { port = 8000 } })

  autoscaling {
    min_pool_size     = 0
    initial_pool_size = 0
    max_pool_size     = 1
  }

  service {
    name        = "server"
    target_port = 8000
    protocol    = "TCP"
  }
}

resource "fleets_github_trust_policy" "cua_cli_wif_smoke" {
  name       = "cua-cli-wif-smoke"
  repository = "trycua/cua"

  allowed_namespaces = [
    fleets_pool.cua_cli_wif_smoke.name,
  ]

  enabled = true
}
