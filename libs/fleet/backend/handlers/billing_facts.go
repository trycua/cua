package handlers

import (
	"context"
	"fmt"
	"net/http"

	"cyclops-cs-backend/auth"
)

func StripeCardFacts(handlers Handlers) auth.FactProvider {
	return stripeCardFacts{handlers: handlers}
}

type stripeCardFacts struct{ handlers Handlers }

func (stripeCardFacts) CacheKey() string { return auth.StripeCardsFactProvider }

func (facts stripeCardFacts) LoadFacts(ctx context.Context, request *http.Request) (auth.FactSet, error) {
	if facts.handlers.Billing == nil {
		return nil, fmt.Errorf("billing service is not configured")
	}
	user := auth.GetUser(request.Context())
	if user == nil || user.ID == "" {
		return nil, fmt.Errorf("Stripe card fact requires an authenticated user subject")
	}
	cards, err := facts.handlers.Billing.AttachedCards(ctx, user.ID)
	if err != nil {
		return nil, &auth.FactUnavailableError{
			Namespace: auth.StripeCardsFactNamespace,
			Err:       fmt.Errorf("Stripe attached-card lookup failed"),
		}
	}
	projected := make([]map[string]any, 0, len(cards))
	for _, card := range cards {
		projected = append(projected, map[string]any{
			"exp_year":  card.ExpYear,
			"exp_month": card.ExpMonth,
		})
	}
	return auth.FactSet{"cards": projected}, nil
}
