---
page_title: "Provider: Cyclops"
description: |-
  Manage Cyclops computer-use pools with Terraform.
---

# Cyclops Provider

The Cyclops provider creates and manages tenant-scoped computer-use pools through the Cyclops API.

```terraform
terraform {
  required_providers {
    cyclops = {
      source = "trycua/cyclops"
    }
  }
}

provider "cyclops" {
  endpoint      = "https://cyclops.example.com"
  client_id     = var.cyclops_client_id
  client_secret = var.cyclops_client_secret
  token_url     = var.cyclops_token_url
}
```

Use either `access_token`, or all three OAuth user-key fields: `client_id`, `client_secret`, and `token_url`. The corresponding `CYCLOPS_*` environment variables may be used instead.
