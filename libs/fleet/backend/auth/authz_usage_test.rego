package authz_usage_test

import rego.v1
import data.authz_base
import data.authz_usage

route_allow if { authz_base.allow; authz_usage.allow }

test_spa_get_allowed if { route_allow with input as {"route":"/api/usage/overview","method":"GET","path":"/api/usage/overview","params":{},"user":{"sub":"user-1","azp":"cyclops-cs-spa","namespace":"","email":""}} }
test_spa_post_denied if { not route_allow with input as {"route":"/api/usage/overview","method":"POST","path":"/api/usage/overview","params":{},"user":{"sub":"user-1","azp":"cyclops-cs-spa","namespace":"","email":""}} }
test_cli_get_denied if { not route_allow with input as {"route":"/api/usage/overview","method":"GET","path":"/api/usage/overview","params":{},"user":{"sub":"user-1","azp":"cua-cli","namespace":"","email":""}} }

test_spa_browser_timings_post_allowed if { route_allow with input as {"route":"/api/usage/browser-timings","method":"POST","path":"/api/usage/browser-timings","params":{},"user":{"sub":"user-1","azp":"cyclops-cs-spa","namespace":"","email":""}} }
