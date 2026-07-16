package handlers

import (
	"context"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
	"time"

	"cyclops-cs-backend/auth"
	"cyclops-cs-backend/templates"
)

// fakeTemplateStore is an in-memory templates.Store for handler tests —
// it exercises the real handler logic (auth, validation, status codes,
// per-user scoping) without a Redis dependency.
type fakeTemplateStore struct {
	m map[string]*templates.Template // keyed by user + "\x00" + name
}

func newFakeStore() *fakeTemplateStore {
	return &fakeTemplateStore{m: map[string]*templates.Template{}}
}

func (f *fakeTemplateStore) key(user, name string) string { return user + "\x00" + name }

func (f *fakeTemplateStore) Save(_ context.Context, t *templates.Template) error {
	if t.CreatedAt.IsZero() {
		t.CreatedAt = time.Now().UTC()
	}
	cp := *t
	f.m[f.key(t.User, t.Name)] = &cp
	return nil
}

func (f *fakeTemplateStore) Get(_ context.Context, user, name string) (*templates.Template, error) {
	t, ok := f.m[f.key(user, name)]
	if !ok {
		return nil, nil
	}
	cp := *t
	return &cp, nil
}

func (f *fakeTemplateStore) List(_ context.Context, user string) ([]*templates.Template, error) {
	var out []*templates.Template
	for _, t := range f.m {
		if t.User == user {
			cp := *t
			out = append(out, &cp)
		}
	}
	return out, nil
}

func (f *fakeTemplateStore) Delete(_ context.Context, user, name string) (bool, error) {
	k := f.key(user, name)
	if _, ok := f.m[k]; !ok {
		return false, nil
	}
	delete(f.m, k)
	return true, nil
}

func newReq(method, target, body string, user *auth.User) *http.Request {
	var r *http.Request
	if body != "" {
		r = httptest.NewRequest(method, target, strings.NewReader(body))
	} else {
		r = httptest.NewRequest(method, target, nil)
	}
	if user != nil {
		r = withUser(r, user)
	}
	return r
}

var alice = &auth.User{ID: "user-alice"}

func TestCreatePoolTemplate_Success(t *testing.T) {
	h := Handlers{Templates: newFakeStore()}
	r := newReq(http.MethodPost, "/api/pool-templates",
		`{"name":"gpu-large","config":{"cpu":8,"ram":"16Gi"}}`, alice)
	w := httptest.NewRecorder()

	h.CreatePoolTemplate(w, r)

	if w.Code != http.StatusCreated {
		t.Fatalf("status = %d, want 201; body = %s", w.Code, w.Body.String())
	}
	var resp PoolTemplateResponse
	if err := json.Unmarshal(w.Body.Bytes(), &resp); err != nil {
		t.Fatalf("decode: %v", err)
	}
	if resp.Name != "gpu-large" || resp.User != "user-alice" {
		t.Fatalf("resp = %+v", resp)
	}
	if resp.CreatedAt == "" {
		t.Fatal("createdAt not set")
	}
	if string(resp.Config) != `{"cpu":8,"ram":"16Gi"}` {
		t.Fatalf("config = %s", string(resp.Config))
	}
}

func TestCreatePoolTemplate_Validation(t *testing.T) {
	cases := map[string]string{
		"bad name":       `{"name":"bad name","config":{}}`,
		"colon in name":  `{"name":"a:b","config":{}}`,
		"empty name":     `{"name":"","config":{}}`,
		"missing config": `{"name":"ok"}`,
		"bad json":       `{"name":"ok","config":}`,
	}
	for label, body := range cases {
		t.Run(label, func(t *testing.T) {
			h := Handlers{Templates: newFakeStore()}
			w := httptest.NewRecorder()
			h.CreatePoolTemplate(w, newReq(http.MethodPost, "/api/pool-templates", body, alice))
			if w.Code != http.StatusBadRequest {
				t.Fatalf("status = %d, want 400; body = %s", w.Code, w.Body.String())
			}
		})
	}
}

func TestPoolTemplate_Unauthorized(t *testing.T) {
	h := Handlers{Templates: newFakeStore()}
	w := httptest.NewRecorder()
	h.ListPoolTemplates(w, newReq(http.MethodGet, "/api/pool-templates", "", nil))
	if w.Code != http.StatusUnauthorized {
		t.Fatalf("status = %d, want 401", w.Code)
	}
}

func TestPoolTemplate_Disabled503(t *testing.T) {
	h := Handlers{} // no store
	w := httptest.NewRecorder()
	h.ListPoolTemplates(w, newReq(http.MethodGet, "/api/pool-templates", "", alice))
	if w.Code != http.StatusServiceUnavailable {
		t.Fatalf("status = %d, want 503; body = %s", w.Code, w.Body.String())
	}
}

func TestListPoolTemplates_IsolatedPerUser(t *testing.T) {
	store := newFakeStore()
	h := Handlers{Templates: store}

	// Alice saves one.
	w := httptest.NewRecorder()
	h.CreatePoolTemplate(w, newReq(http.MethodPost, "/api/pool-templates",
		`{"name":"a-tpl","config":{"cpu":1}}`, alice))
	if w.Code != http.StatusCreated {
		t.Fatalf("alice create: %d", w.Code)
	}

	// Bob lists — must not see Alice's template.
	bob := &auth.User{ID: "user-bob"}
	w = httptest.NewRecorder()
	h.ListPoolTemplates(w, newReq(http.MethodGet, "/api/pool-templates", "", bob))
	if w.Code != http.StatusOK {
		t.Fatalf("bob list: %d", w.Code)
	}
	var bobList []PoolTemplateResponse
	_ = json.Unmarshal(w.Body.Bytes(), &bobList)
	if len(bobList) != 0 {
		t.Fatalf("bob saw %d templates, want 0", len(bobList))
	}

	// Alice lists — sees exactly her one template.
	w = httptest.NewRecorder()
	h.ListPoolTemplates(w, newReq(http.MethodGet, "/api/pool-templates", "", alice))
	var aliceList []PoolTemplateResponse
	_ = json.Unmarshal(w.Body.Bytes(), &aliceList)
	if len(aliceList) != 1 || aliceList[0].Name != "a-tpl" {
		t.Fatalf("alice list = %+v", aliceList)
	}
}

func TestGetPoolTemplate_NotFound(t *testing.T) {
	h := Handlers{Templates: newFakeStore()}
	r := newReq(http.MethodGet, "/api/pool-templates/missing", "", alice)
	r.SetPathValue("name", "missing")
	w := httptest.NewRecorder()
	h.GetPoolTemplate(w, r)
	if w.Code != http.StatusNotFound {
		t.Fatalf("status = %d, want 404", w.Code)
	}
}

func TestDeletePoolTemplate_RoundTrip(t *testing.T) {
	store := newFakeStore()
	h := Handlers{Templates: store}

	// Create, then delete → 204, then delete again → 404.
	w := httptest.NewRecorder()
	h.CreatePoolTemplate(w, newReq(http.MethodPost, "/api/pool-templates",
		`{"name":"doomed","config":{"cpu":1}}`, alice))
	if w.Code != http.StatusCreated {
		t.Fatalf("create: %d", w.Code)
	}

	del := func() int {
		r := newReq(http.MethodDelete, "/api/pool-templates/doomed", "", alice)
		r.SetPathValue("name", "doomed")
		w := httptest.NewRecorder()
		h.DeletePoolTemplate(w, r)
		return w.Code
	}
	if code := del(); code != http.StatusNoContent {
		t.Fatalf("first delete = %d, want 204", code)
	}
	if code := del(); code != http.StatusNotFound {
		t.Fatalf("second delete = %d, want 404", code)
	}
}
