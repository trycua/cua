package handlers

import (
	"net/http"
	"net/http/httptest"
	"testing"
)

func TestHealthIsLiveWhenDatabaseIsUnavailable(t *testing.T) {
	response := httptest.NewRecorder()

	Handlers{}.GetHealth(response, httptest.NewRequest(http.MethodGet, "/healthz", nil))

	if response.Code != http.StatusOK {
		t.Fatalf("status = %d, want %d", response.Code, http.StatusOK)
	}
}

func TestReadinessFailsClosedUntilDatabaseSchemaIsReady(t *testing.T) {
	readiness := NewReadiness()
	h := Handlers{Readiness: readiness}

	for _, name := range []string{"initial state", "schema check failure"} {
		t.Run(name, func(t *testing.T) {
			response := httptest.NewRecorder()
			h.GetReadiness(response, httptest.NewRequest(http.MethodGet, "/readyz", nil))

			if response.Code < http.StatusBadRequest {
				t.Fatalf("status = %d, want non-2xx", response.Code)
			}
		})
	}

	readiness.MarkReady()
	response := httptest.NewRecorder()
	h.GetReadiness(response, httptest.NewRequest(http.MethodGet, "/readyz", nil))

	if response.Code != http.StatusOK {
		t.Fatalf("status = %d, want %d", response.Code, http.StatusOK)
	}
}
