package authz_images_resolve_test

import rego.v1
import data.authz_base
import data.authz_images_resolve

route_allow if { authz_base.allow; authz_images_resolve.allow }

resolve(user) := {"route": "/api/images/resolve", "method": "GET", "path": "/api/images/resolve", "params": {}, "user": user}

test_spa_resolve_allowed if { route_allow with input as resolve({"sub": "user-1", "azp": "cyclops-cs-spa", "namespace": "", "email": ""}) }

test_user_key_resolve_allowed if { route_allow with input as resolve({"sub": "user-1", "azp": "ukey-abc", "namespace": "", "email": ""}) }

test_github_oidc_resolve_allowed if { route_allow with input as resolve({"sub": "repo:org/repo", "azp": "github", "principal_type": "github_oidc", "allowed_namespaces": ["ns-a"]}) }

test_other_route_denied if { not authz_images_resolve.allow with input as object.union(resolve({"sub": "user-1", "azp": "cyclops-cs-spa"}), {"route": "/api/image-uploads/presign"}) }
