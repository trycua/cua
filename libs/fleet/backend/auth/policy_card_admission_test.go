package auth

import (
	"context"
	"errors"
	"net/http"
	"net/http/httptest"
	"sync/atomic"
	"testing"

	"github.com/trycua/cloud/pkg/featureflags"
)

type countingAdmissionFacts struct {
	cacheKey    string
	namespace   string
	facts       FactSet
	loads       atomic.Int64
	unavailable bool
}

func (facts *countingAdmissionFacts) CacheKey() string { return facts.cacheKey }

func (facts *countingAdmissionFacts) LoadFacts(context.Context, *http.Request) (FactSet, error) {
	facts.loads.Add(1)
	if facts.unavailable {
		return nil, &FactUnavailableError{Namespace: facts.namespace, Err: errors.New("dependency unavailable")}
	}
	return facts.facts, nil
}

func installAdmissionFacts(t *testing.T, name string, provider FactProvider) {
	t.Helper()
	factProviderRegistry.RLock()
	previous, existed := factProviderRegistry.providers[name]
	factProviderRegistry.RUnlock()
	RegisterFactProvider(name, provider)
	t.Cleanup(func() {
		if existed {
			RegisterFactProvider(name, previous)
			return
		}
		factProviderRegistry.Lock()
		delete(factProviderRegistry.providers, name)
		factProviderRegistry.Unlock()
	})
}

func setCardAdmissionFlags(t *testing.T, enabled bool, admins ...string) {
	t.Helper()
	if err := featureflags.SetupProvider(context.Background(), "development", featureflags.AWSCredentials{}); err != nil {
		t.Fatalf("setup feature flags: %v", err)
	}
	if enabled {
		t.Setenv("CYCLOPS_CS_REQUIRE_CARD_FOR_CUSTOM_RESOURCE_CREATION", "true")
	} else {
		t.Setenv("CYCLOPS_CS_REQUIRE_CARD_FOR_CUSTOM_RESOURCE_CREATION", "false")
	}
	adminJSON := `[]`
	if len(admins) > 0 {
		adminJSON = `["` + admins[0] + `"]`
	}
	t.Setenv("CYCLOPS_CS_ADMIN_SUBS", adminJSON)
	resetFlagsCache()
	LoadOpa()
}

func cardAdmissionRequest(method, path, sub string) *http.Request {
	request := routeRequest(method, "/api/k8s/"+path, "/api/k8s/{path...}", map[string]string{"path": path}, `{}`)
	ctx := context.WithValue(request.Context(), UserKey, &User{ID: sub, AZP: "cyclops-cs-spa", PrincipalType: PrincipalTypeUser})
	return request.WithContext(ctx)
}

func runK8sPolicy(t *testing.T, request *http.Request) (int, bool) {
	t.Helper()
	reached := false
	handler := PolicyMiddleware(K8sRoutePolicy())(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
		reached = true
		w.WriteHeader(http.StatusNoContent)
	}))
	response := httptest.NewRecorder()
	handler.ServeHTTP(response, request)
	return response.Code, reached
}

func runK8sPolicyResponse(t *testing.T, request *http.Request) (*httptest.ResponseRecorder, bool) {
	t.Helper()
	surface, ok := surfacePolicies["k8s"]
	if !ok {
		t.Fatal("K8s surface is not configured")
	}
	options := append([]MiddlewareOption{}, surface.options...)
	options = append(options, WithPipeline())

	reached := false
	handler := PolicyMiddleware(surface.tree(), options...)(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
		reached = true
		w.WriteHeader(http.StatusNoContent)
	}))
	response := httptest.NewRecorder()
	handler.ServeHTTP(response, request)
	return response, reached
}

func TestK8sCopyTestsUseProductionSurfaceDeniedMessage(t *testing.T) {
	const want = "configured K8s denial"
	previous := surfacePolicies["k8s"]
	surface := previous
	surface.options = []MiddlewareOption{WithDeniedMessage(want)}
	surfacePolicies["k8s"] = surface
	t.Cleanup(func() {
		surfacePolicies["k8s"] = previous
	})

	setCardAdmissionFlags(t, false)
	response, reached := runK8sPolicyResponse(t, cardAdmissionRequest(http.MethodGet, "api/v1/nodes", "user-1"))
	if response.Code != http.StatusForbidden || reached {
		t.Fatalf("status, reached = %d, %v; want %d, false", response.Code, reached, http.StatusForbidden)
	}
	if got := policyErrorMessage(t, response); got != want {
		t.Fatalf("error = %q, want %q", got, want)
	}
}

func TestK8sCardAdmissionReturnsBillingMessage(t *testing.T) {
	setCardAdmissionFlags(t, true)
	installAdmissionFacts(t, StripeCardsFactProvider, &countingAdmissionFacts{
		cacheKey: StripeCardsFactProvider,
		facts:    FactSet{"cards": []map[string]any{}},
	})
	installAdmissionFacts(t, CurrentYearFactProvider, &countingAdmissionFacts{
		cacheKey: CurrentYearFactProvider,
		facts:    FactSet{"current_year": 2026},
	})
	installAdmissionFacts(t, CurrentMonthFactProvider, &countingAdmissionFacts{
		cacheKey: CurrentMonthFactProvider,
		facts:    FactSet{"current_month": 8},
	})

	const path = "apis/osgym.cua.ai/v1alpha1/namespaces/ns-a/osgymsandboxclaims"
	response, reached := runK8sPolicyResponse(t, cardAdmissionRequest(http.MethodPost, path, "user-1"))
	if response.Code != http.StatusForbidden || reached {
		t.Fatalf("status, reached = %d, %v; want %d, false", response.Code, reached, http.StatusForbidden)
	}
	const want = BillingSetupRequiredMessage
	if got := policyErrorMessage(t, response); got != want {
		t.Fatalf("error = %q, want %q", got, want)
	}
}

func TestK8sUnrelatedDenialKeepsSurfaceMessage(t *testing.T) {
	setCardAdmissionFlags(t, false)
	const path = "api/v1/nodes"
	response, reached := runK8sPolicyResponse(t, cardAdmissionRequest(http.MethodGet, path, "user-1"))
	if response.Code != http.StatusForbidden || reached {
		t.Fatalf("status, reached = %d, %v; want %d, false", response.Code, reached, http.StatusForbidden)
	}
	if got := policyErrorMessage(t, response); got != "k8s request is not allowed" {
		t.Fatalf("error = %q, want %q", got, "k8s request is not allowed")
	}
}

func TestK8sCardAdmissionLazyTruthTable(t *testing.T) {
	const createPath = "apis/osgym.cua.ai/v1alpha1/namespaces/ns-a/osgymsandboxclaims"
	const readPath = "apis/osgym.cua.ai/v1alpha1/namespaces/ns-a/osgymsandboxclaims/claim-a"

	cases := []struct {
		name              string
		enabled           bool
		admins            []string
		exemptSubs        []string
		user              string
		method            string
		path              string
		cards             []map[string]any
		stripeUnavailable bool
		wantStatus        int
		wantReached       bool
		wantStripeLoads   int64
		wantYearLoads     int64
		wantMonthLoads    int64
	}{
		{name: "flag disabled bypass", enabled: false, user: "user-1", method: http.MethodPost, path: createPath, wantStatus: http.StatusNoContent, wantReached: true},
		{name: "admin bypass", enabled: true, admins: []string{"admin-1"}, user: "admin-1", method: http.MethodPost, path: createPath, stripeUnavailable: true, wantStatus: http.StatusNoContent, wantReached: true},
		{name: "configured non-admin bypass", enabled: true, exemptSubs: []string{"exempt-user"}, user: "exempt-user", method: http.MethodPost, path: createPath, stripeUnavailable: true, wantStatus: http.StatusNoContent, wantReached: true},
		{name: "different non-admin is not exempt", enabled: true, exemptSubs: []string{"exempt-user"}, user: "user-1", method: http.MethodPost, path: createPath, wantStatus: http.StatusForbidden, wantStripeLoads: 1, wantYearLoads: 1, wantMonthLoads: 1},
		{name: "email-like noncanonical identity is not admin", enabled: true, admins: []string{"admin@example.com"}, user: "user-1", method: http.MethodPost, path: createPath, wantStatus: http.StatusForbidden, wantStripeLoads: 1, wantYearLoads: 1, wantMonthLoads: 1},
		{name: "non-create bypass", enabled: true, user: "user-1", method: http.MethodGet, path: readPath, stripeUnavailable: true, wantStatus: http.StatusNoContent, wantReached: true},
		{name: "current-month card allows", enabled: true, user: "user-1", method: http.MethodPost, path: createPath, cards: []map[string]any{{"exp_year": 2026, "exp_month": 8}}, wantStatus: http.StatusNoContent, wantReached: true, wantStripeLoads: 1, wantYearLoads: 1, wantMonthLoads: 1},
		{name: "empty cards deny", enabled: true, user: "user-1", method: http.MethodPost, path: createPath, wantStatus: http.StatusForbidden, wantStripeLoads: 1, wantYearLoads: 1, wantMonthLoads: 1},
		{name: "stripe outage fails closed", enabled: true, user: "user-1", method: http.MethodPost, path: createPath, stripeUnavailable: true, wantStatus: http.StatusBadGateway, wantStripeLoads: 1},
	}

	for _, testCase := range cases {
		t.Run(testCase.name, func(t *testing.T) {
			setCardAdmissionFlags(t, testCase.enabled, testCase.admins...)
			if len(testCase.exemptSubs) > 0 {
				t.Setenv("CYCLOPS_CS_CARD_REQUIREMENT_EXEMPT_SUBS", `["`+testCase.exemptSubs[0]+`"]`)
				resetFlagsCache()
			}
			stripeCards := &countingAdmissionFacts{cacheKey: StripeCardsFactProvider, namespace: StripeCardsFactNamespace, facts: FactSet{"cards": testCase.cards}, unavailable: testCase.stripeUnavailable}
			currentYear := &countingAdmissionFacts{cacheKey: CurrentYearFactProvider, namespace: TimeFactNamespace, facts: FactSet{"current_year": 2026}}
			currentMonth := &countingAdmissionFacts{cacheKey: CurrentMonthFactProvider, namespace: TimeFactNamespace, facts: FactSet{"current_month": 8}}
			installAdmissionFacts(t, StripeCardsFactProvider, stripeCards)
			installAdmissionFacts(t, CurrentYearFactProvider, currentYear)
			installAdmissionFacts(t, CurrentMonthFactProvider, currentMonth)

			status, reached := runK8sPolicy(t, cardAdmissionRequest(testCase.method, testCase.path, testCase.user))
			if status != testCase.wantStatus || reached != testCase.wantReached {
				t.Fatalf("status, reached = %d, %v; want %d, %v", status, reached, testCase.wantStatus, testCase.wantReached)
			}
			for name, expected := range map[string]struct {
				facts *countingAdmissionFacts
				loads int64
			}{
				"stripe": {stripeCards, testCase.wantStripeLoads},
				"year":   {currentYear, testCase.wantYearLoads},
				"month":  {currentMonth, testCase.wantMonthLoads},
			} {
				if got := expected.facts.loads.Load(); got != expected.loads {
					t.Fatalf("%s fact loads = %d, want %d", name, got, expected.loads)
				}
			}
		})
	}
}

func TestK8sCardAdmissionMissingProvidersFailClosed(t *testing.T) {
	const path = "apis/osgym.cua.ai/v1alpha1/namespaces/ns-a/osgymsandboxclaims"
	for _, missing := range []string{StripeCardsFactProvider, CurrentYearFactProvider, CurrentMonthFactProvider} {
		t.Run(missing, func(t *testing.T) {
			setCardAdmissionFlags(t, true)
			providers := map[string]FactProvider{
				StripeCardsFactProvider:  &countingAdmissionFacts{cacheKey: StripeCardsFactProvider, facts: FactSet{"cards": []map[string]any{{"exp_year": 2027, "exp_month": 1}}}},
				CurrentYearFactProvider:  &countingAdmissionFacts{cacheKey: CurrentYearFactProvider, facts: FactSet{"current_year": 2026}},
				CurrentMonthFactProvider: &countingAdmissionFacts{cacheKey: CurrentMonthFactProvider, facts: FactSet{"current_month": 8}},
			}
			for name, provider := range providers {
				if name != missing {
					installAdmissionFacts(t, name, provider)
				}
			}
			factProviderRegistry.Lock()
			previous, existed := factProviderRegistry.providers[missing]
			delete(factProviderRegistry.providers, missing)
			factProviderRegistry.Unlock()
			t.Cleanup(func() {
				if existed {
					RegisterFactProvider(missing, previous)
				}
			})

			status, reached := runK8sPolicy(t, cardAdmissionRequest(http.MethodPost, path, "user-1"))
			if status != http.StatusInternalServerError || reached {
				t.Fatalf("status, reached = %d, %v; want 500, false", status, reached)
			}
		})
	}
}
