# Admission for direct creation of custom-resource instances through /api/k8s.
package custom_resource_creation_admission

import data.authz

default exempt = false

default has_qualifying_card = false

default billing_eligible = false

custom_api_group(group) {
	group == "cua.ai"
}

custom_api_group(group) {
	group == "osgym.cua.ai"
}

is_custom_resource_create {
	input.method == "POST"
	parts := split(input.params.path, "/")
	count(parts) == 4
	parts[0] == "apis"
	custom_api_group(parts[1])
	parts[3] != ""
}

is_custom_resource_create {
	input.method == "POST"
	parts := split(input.params.path, "/")
	count(parts) == 6
	parts[0] == "apis"
	custom_api_group(parts[1])
	parts[3] == "namespaces"
	parts[4] != ""
	parts[5] != ""
}

exempt {
	object.get(input.flags, "require_card_for_custom_resource_creation", false) != true
}

exempt {
	not is_custom_resource_create
}

exempt {
	authz.is_admin
}

exempt {
	input.flags.card_requirement_exempt_subs[_] == input.user.sub
}

billing_eligible {
	input.facts.stripe_cards.grandfathered == true
}

billing_eligible {
	has_qualifying_card
}

has_qualifying_card {
	current_year := input.facts.time.current_year
	input.facts.time.current_month
	card := input.facts.stripe_cards.cards[_]
	card.exp_year > current_year
}

has_qualifying_card {
	current_year := input.facts.time.current_year
	current_month := input.facts.time.current_month
	card := input.facts.stripe_cards.cards[_]
	card.exp_year == current_year
	card.exp_month >= current_month
}
