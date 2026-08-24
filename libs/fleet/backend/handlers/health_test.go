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

func TestReadinessIsReadyWhenDatabaseIsUnavailable(t *testing.T) {
	response := httptest.NewRecorder()

	Handlers{}.GetReadiness(response, httptest.NewRequest(http.MethodGet, "/readyz", nil))

	if response.Code != http.StatusOK {
		t.Fatalf("status = %d, want %d", response.Code, http.StatusOK)
	}
}
