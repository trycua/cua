package handlers

import (
	"context"

	"cyclops-cs-backend/auth"
)

const databaseRequestTimeout = auth.DatabaseRequestTimeout

func databaseContext(parent context.Context) (context.Context, context.CancelFunc) {
	return auth.DatabaseContext(parent)
}
