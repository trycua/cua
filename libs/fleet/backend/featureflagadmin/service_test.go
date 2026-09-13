package featureflagadmin

import (
	"context"
	"encoding/json"
	"errors"
	"sort"
	"testing"
	"time"

	"github.com/trycua/cloud/pkg/featureflags"
)

func TestServiceListClassifiesOwnershipAndFiltersNestedKeys(t *testing.T) {
	modifiedAt := time.Date(2026, 8, 13, 12, 0, 0, 0, time.UTC)
	store := &fakeStore{parameters: map[string]featureflags.Parameter{
		Prefix + "terraform-flag": {
			Name: Prefix + "terraform-flag", Value: "true", Type: "String", Version: 3, LastModified: modifiedAt,
			Tags: map[string]string{"ManagedBy": "terraform"},
		},
		Prefix + "ad-hoc-flag": {
			Name: Prefix + "ad-hoc-flag", Value: "value", Type: "String", Version: 4, LastModified: modifiedAt,
			Tags: map[string]string{"ManagedBy": "cyclops-cs-admin"},
		},
		Prefix + "external-flag": {
			Name: Prefix + "external-flag", Value: "false", Type: "String", Version: 5, LastModified: modifiedAt,
			Tags: map[string]string{"ManagedBy": "another-system"},
		},
		Prefix + "nested/flag": {
			Name: Prefix + "nested/flag", Value: "ignored", Type: "String", Version: 6, LastModified: modifiedAt,
		},
		Prefix + "secret": {
			Name: Prefix + "secret", Value: "ignored", Type: "SecureString", Version: 7, LastModified: modifiedAt,
		},
	}}

	flags, err := NewService(store, nil, nil, nil).List(context.Background())
	if err != nil {
		t.Fatalf("List() error = %v", err)
	}
	if len(flags) != 3 {
		t.Fatalf("List() returned %d flags, want 3: %#v", len(flags), flags)
	}

	if flags[0].Key != "ad-hoc-flag" || flags[0].Ownership != OwnershipAdHoc || !flags[0].Deletable {
		t.Fatalf("ad hoc flag = %#v", flags[0])
	}
	if flags[1].Key != "external-flag" || flags[1].Ownership != OwnershipExternal || flags[1].Deletable {
		t.Fatalf("external flag = %#v", flags[1])
	}
	if flags[2].Key != "terraform-flag" || flags[2].Ownership != OwnershipTerraform || flags[2].Deletable {
		t.Fatalf("terraform flag = %#v", flags[2])
	}
}

func TestServiceListReturnsTypedValuesAndVersions(t *testing.T) {
	modifiedAt := time.Date(2026, 8, 13, 12, 0, 0, 0, time.UTC)
	store := &fakeStore{parameters: map[string]featureflags.Parameter{
		Prefix + "boolean": {Name: Prefix + "boolean", Value: "true", Type: "String", Version: 1, LastModified: modifiedAt},
		Prefix + "json":    {Name: Prefix + "json", Value: `{"items":[1,"two"]}`, Type: "String", Version: 2, LastModified: modifiedAt},
		Prefix + "number":  {Name: Prefix + "number", Value: "42.5", Type: "String", Version: 3, LastModified: modifiedAt},
		Prefix + "string":  {Name: Prefix + "string", Value: "hello", Type: "String", Version: 4, LastModified: modifiedAt},
	}}

	flags, err := NewService(store, nil, nil, nil).List(context.Background())
	if err != nil {
		t.Fatalf("List() error = %v", err)
	}
	byKey := map[string]Flag{}
	for _, flag := range flags {
		byKey[flag.Key] = flag
	}

	if got := byKey["boolean"]; got.ValueType != featureflags.ValueBoolean || got.Value != true || got.RawValue != "true" || got.Version != 1 || !got.ModifiedAt.Equal(modifiedAt) {
		t.Fatalf("boolean flag = %#v", got)
	}
	if got := byKey["number"]; got.ValueType != featureflags.ValueNumber || got.RawValue != "42.5" || got.Version != 3 {
		t.Fatalf("number flag = %#v", got)
	}
	if got := byKey["string"]; got.ValueType != featureflags.ValueString || got.Value != "hello" || got.RawValue != "hello" || got.Version != 4 {
		t.Fatalf("string flag = %#v", got)
	}
	jsonFlag := byKey["json"]
	if jsonFlag.ValueType != featureflags.ValueJSON || jsonFlag.RawValue != `{"items":[1,"two"]}` || jsonFlag.Version != 2 {
		t.Fatalf("json flag = %#v", jsonFlag)
	}
	encoded, err := json.Marshal(jsonFlag.Value)
	if err != nil || string(encoded) != `{"items":[1,"two"]}` {
		t.Fatalf("json flag value = %s, %v", encoded, err)
	}
}

type fakeStore struct {
	parameters map[string]featureflags.Parameter
	updateErr  error
	deleteErr  error
}

func (s *fakeStore) List(_ context.Context, prefix string) ([]featureflags.Parameter, error) {
	parameters := make([]featureflags.Parameter, 0, len(s.parameters))
	for path, parameter := range s.parameters {
		if len(path) >= len(prefix) && path[:len(prefix)] == prefix {
			parameters = append(parameters, cloneParameter(parameter))
		}
	}
	sort.Slice(parameters, func(i, j int) bool { return parameters[i].Name < parameters[j].Name })
	return parameters, nil
}

func (s *fakeStore) Get(_ context.Context, path string) (featureflags.Parameter, error) {
	parameter, ok := s.parameters[path]
	if !ok {
		return featureflags.Parameter{}, featureflags.ErrParameterNotFound
	}
	return cloneParameter(parameter), nil
}

func (s *fakeStore) Create(_ context.Context, parameter featureflags.Parameter) (featureflags.Parameter, error) {
	if _, exists := s.parameters[parameter.Name]; exists {
		return featureflags.Parameter{}, featureflags.ErrParameterExists
	}
	parameter.Version = 1
	s.parameters[parameter.Name] = cloneParameter(parameter)
	return cloneParameter(parameter), nil
}

func (s *fakeStore) Update(_ context.Context, path, value string) (featureflags.Parameter, error) {
	updateErr := s.updateErr
	if updateErr != nil {
		return featureflags.Parameter{}, errors.Join(updateErr)
	}
	parameter, ok := s.parameters[path]
	if !ok {
		return featureflags.Parameter{}, featureflags.ErrParameterNotFound
	}
	parameter.Value = value
	parameter.Version++
	s.parameters[path] = cloneParameter(parameter)
	return cloneParameter(parameter), nil
}

func (s *fakeStore) Delete(_ context.Context, path string) error {
	deleteErr := s.deleteErr
	if deleteErr != nil {
		return errors.Join(deleteErr)
	}
	if _, ok := s.parameters[path]; !ok {
		return featureflags.ErrParameterNotFound
	}
	delete(s.parameters, path)
	return nil
}

func TestServiceMapsSecureStringMutationErrors(t *testing.T) {
	parameter := featureflags.Parameter{Name: Prefix + "secret", Value: "opaque", Type: "String", Version: 4, Tags: map[string]string{"ManagedBy": "cyclops-cs-admin"}}
	for _, testCase := range []struct {
		name  string
		run   func(*Service) error
		store *fakeStore
	}{
		{name: "update", store: &fakeStore{parameters: map[string]featureflags.Parameter{parameter.Name: parameter}, updateErr: featureflags.ErrSecureString}, run: func(service *Service) error {
			_, err := service.Update(context.Background(), Actor{}, "secret", UpdateInput{ValueType: featureflags.ValueString, Value: "replacement", ExpectedVersion: 4})
			return err
		}},
		{name: "delete", store: &fakeStore{parameters: map[string]featureflags.Parameter{parameter.Name: parameter}, deleteErr: featureflags.ErrSecureString}, run: func(service *Service) error {
			return service.Delete(context.Background(), Actor{}, "secret", 4)
		}},
	} {
		t.Run(testCase.name, func(t *testing.T) {
			err := testCase.run(NewService(testCase.store, &fakeLock{}, nil, nil))
			var serviceError *ServiceError
			if !errors.As(err, &serviceError) {
				t.Fatalf("error = %v, want ServiceError", err)
			}
			if serviceError.Code != "unsupported_parameter" || serviceError.HTTPStatus != 422 {
				t.Fatalf("error = %#v", serviceError)
			}
		})
	}
}

func cloneParameter(parameter featureflags.Parameter) featureflags.Parameter {
	cloned := parameter
	if parameter.Tags != nil {
		cloned.Tags = make(map[string]string, len(parameter.Tags))
		for key, value := range parameter.Tags {
			cloned.Tags[key] = value
		}
	}
	return cloned
}

type fakeLock struct {
	calls int
	err   error
}

func (l *fakeLock) WithLock(ctx context.Context, callback func(context.Context) error) error {
	l.calls++
	lockErr := l.err
	if lockErr != nil {
		return errors.Join(lockErr)
	}
	return callback(ctx)
}

type fakeInvalidator struct{ calls int }

func (i *fakeInvalidator) InvalidateFeatureFlags() { i.calls++ }

type fakeAuditLogger struct{ events []AuditEvent }

func (l *fakeAuditLogger) Log(_ context.Context, event AuditEvent) {
	l.events = append(l.events, event)
}

func TestServiceCreateTagsAndInvalidatesAfterSuccess(t *testing.T) {
	store := &fakeStore{parameters: map[string]featureflags.Parameter{}}
	lock := &fakeLock{}
	invalidator := &fakeInvalidator{}
	audit := &fakeAuditLogger{}
	actor := Actor{Subject: "admin", Email: "admin@example.com", PrincipalType: "spa", TraceID: "trace-1"}

	flag, err := NewService(store, lock, invalidator, audit).Create(context.Background(), actor, CreateInput{
		Key: "new-flag", ValueType: featureflags.ValueJSON, Value: map[string]any{"enabled": true}, Description: "new flag",
	})
	if err != nil {
		t.Fatalf("Create() error = %v", err)
	}
	parameter := store.parameters[Prefix+"new-flag"]
	if parameter.Value != `{"enabled":true}` || parameter.Description != "new flag" {
		t.Fatalf("created parameter = %#v", parameter)
	}
	wantTags := map[string]string{"ManagedBy": "cyclops-cs-admin", "Feature": "cyclops-cs", "Environment": "production"}
	if !equalTags(parameter.Tags, wantTags) {
		t.Fatalf("created tags = %#v, want %#v", parameter.Tags, wantTags)
	}
	if flag.Ownership != OwnershipAdHoc || !flag.Deletable || invalidator.calls != 1 || lock.calls != 1 {
		t.Fatalf("Create() flag=%#v invalidations=%d locks=%d", flag, invalidator.calls, lock.calls)
	}
	if len(audit.events) != 1 || audit.events[0].Result != "success" || audit.events[0].ResultVersion != 1 {
		t.Fatalf("audit events = %#v", audit.events)
	}
}

func TestServiceCreateRejectsDuplicateWithoutInvalidation(t *testing.T) {
	store := &fakeStore{parameters: map[string]featureflags.Parameter{
		Prefix + "existing": {Name: Prefix + "existing", Value: "true", Type: "String", Version: 1},
	}}
	invalidator := &fakeInvalidator{}
	audit := &fakeAuditLogger{}

	_, err := NewService(store, &fakeLock{}, invalidator, audit).Create(context.Background(), Actor{}, CreateInput{
		Key: "existing", ValueType: featureflags.ValueBoolean, Value: true,
	})
	assertServiceError(t, err, "flag_exists", 409)
	if invalidator.calls != 0 {
		t.Fatalf("invalidations = %d, want 0", invalidator.calls)
	}
	if len(audit.events) != 1 || audit.events[0].Result != "rejected" || audit.events[0].Reason != "flag_exists" {
		t.Fatalf("audit events = %#v", audit.events)
	}
}

func TestServiceMapsMissingFlagToStableError(t *testing.T) {
	store := &fakeStore{parameters: map[string]featureflags.Parameter{}}
	_, err := NewService(store, &fakeLock{}, nil, nil).Update(context.Background(), Actor{}, "missing", UpdateInput{
		ValueType: featureflags.ValueBoolean, Value: true, ExpectedVersion: 1,
	})
	assertServiceError(t, err, "flag_not_found", 404)
}

func TestServiceRejectsInvalidKeysBeforeStoreCalls(t *testing.T) {
	for _, key := range []string{"", "UPPER", "nested/key", Prefix + "escape", string(make([]byte, 64))} {
		t.Run(key, func(t *testing.T) {
			store := &fakeStore{parameters: map[string]featureflags.Parameter{}}
			_, err := NewService(store, &fakeLock{}, nil, nil).Create(context.Background(), Actor{}, CreateInput{
				Key: key, ValueType: featureflags.ValueString, Value: "value",
			})
			assertServiceError(t, err, "invalid_key", 400)
			if len(store.parameters) != 0 {
				t.Fatalf("store was changed for invalid key %q", key)
			}
		})
	}
}

func TestServiceUpdateAllowsTerraformAndExternalOwnership(t *testing.T) {
	for _, managedBy := range []string{"terraform", "another-system"} {
		t.Run(managedBy, func(t *testing.T) {
			store := &fakeStore{parameters: map[string]featureflags.Parameter{
				Prefix + "flag": {Name: Prefix + "flag", Value: "old", Type: "String", Version: 4, Tags: map[string]string{"ManagedBy": managedBy}},
			}}
			invalidator := &fakeInvalidator{}
			flag, err := NewService(store, &fakeLock{}, invalidator, nil).Update(context.Background(), Actor{}, "flag", UpdateInput{
				ValueType: featureflags.ValueString, Value: "new", ExpectedVersion: 4,
			})
			if err != nil {
				t.Fatalf("Update() error = %v", err)
			}
			if flag.Version != 5 || store.parameters[Prefix+"flag"].Value != "new" || invalidator.calls != 1 {
				t.Fatalf("updated flag=%#v parameter=%#v invalidations=%d", flag, store.parameters[Prefix+"flag"], invalidator.calls)
			}
		})
	}
}

func TestServiceUpdateRejectsStaleVersionWithoutInvalidation(t *testing.T) {
	store := &fakeStore{parameters: map[string]featureflags.Parameter{
		Prefix + "flag": {Name: Prefix + "flag", Value: "old", Type: "String", Version: 4},
	}}
	invalidator := &fakeInvalidator{}
	audit := &fakeAuditLogger{}

	_, err := NewService(store, &fakeLock{}, invalidator, audit).Update(context.Background(), Actor{}, "flag", UpdateInput{
		ValueType: featureflags.ValueString, Value: "new", ExpectedVersion: 3,
	})
	serviceError := assertServiceError(t, err, "version_conflict", 409)
	if serviceError.Current == nil || serviceError.Current.Version != 4 || store.parameters[Prefix+"flag"].Value != "old" || invalidator.calls != 0 {
		t.Fatalf("conflict=%#v parameter=%#v invalidations=%d", serviceError, store.parameters[Prefix+"flag"], invalidator.calls)
	}
	if len(audit.events) != 1 || audit.events[0].PreviousVersion != 4 || audit.events[0].Reason != "version_conflict" {
		t.Fatalf("audit events = %#v", audit.events)
	}
}

func TestServiceDeleteAllowsAdHocAndProtectsOtherOwnership(t *testing.T) {
	for _, testCase := range []struct {
		name      string
		managedBy string
		wantError string
	}{
		{name: "ad hoc", managedBy: "cyclops-cs-admin"},
		{name: "terraform", managedBy: "terraform", wantError: "managed_flag"},
		{name: "external", managedBy: "other", wantError: "unknown_ownership"},
	} {
		t.Run(testCase.name, func(t *testing.T) {
			store := &fakeStore{parameters: map[string]featureflags.Parameter{
				Prefix + "flag": {Name: Prefix + "flag", Value: "old", Type: "String", Version: 2, Tags: map[string]string{"ManagedBy": testCase.managedBy}},
			}}
			invalidator := &fakeInvalidator{}
			err := NewService(store, &fakeLock{}, invalidator, nil).Delete(context.Background(), Actor{}, "flag", 2)
			if testCase.wantError != "" {
				assertServiceError(t, err, testCase.wantError, 422)
				if invalidator.calls != 0 {
					t.Fatalf("invalidations = %d, want 0", invalidator.calls)
				}
				return
			}
			if err != nil || invalidator.calls != 1 {
				t.Fatalf("Delete() error=%v invalidations=%d", err, invalidator.calls)
			}
			if _, exists := store.parameters[Prefix+"flag"]; exists {
				t.Fatal("ad hoc flag still exists after delete")
			}
		})
	}
}

func TestServiceNeverDeletesAdminSubs(t *testing.T) {
	store := &fakeStore{parameters: map[string]featureflags.Parameter{
		Prefix + "admin-subs": {Name: Prefix + "admin-subs", Value: `["actor"]`, Type: "String", Version: 2, Tags: map[string]string{"ManagedBy": "cyclops-cs-admin"}},
	}}
	audit := &fakeAuditLogger{}
	err := NewService(store, &fakeLock{}, nil, audit).Delete(context.Background(), Actor{Subject: "actor"}, "admin-subs", 2)
	assertServiceError(t, err, "last_admin", 422)
	if _, exists := store.parameters[Prefix+"admin-subs"]; !exists {
		t.Fatal("admin-subs was deleted")
	}
	if len(audit.events) != 1 || audit.events[0].Reason != "last_admin" {
		t.Fatalf("audit events = %#v", audit.events)
	}
}

func TestServiceDeleteSuccessAuditsNoResultVersion(t *testing.T) {
	store := &fakeStore{parameters: map[string]featureflags.Parameter{
		Prefix + "flag": {Name: Prefix + "flag", Value: "true", Type: "String", Version: 7, Tags: map[string]string{"ManagedBy": "cyclops-cs-admin"}},
	}}
	audit := &fakeAuditLogger{}
	if err := NewService(store, &fakeLock{}, nil, audit).Delete(context.Background(), Actor{}, "flag", 7); err != nil {
		t.Fatal(err)
	}
	if len(audit.events) != 1 || audit.events[0].ResultVersion != 0 || audit.events[0].PreviousVersion != 7 {
		t.Fatalf("delete audit = %#v", audit.events)
	}
}

func TestUpdateAdminSubsAllowsSelfRemovalWhenAnotherAdminRemains(t *testing.T) {
	store := &fakeStore{parameters: map[string]featureflags.Parameter{
		Prefix + "admin-subs": {Name: Prefix + "admin-subs", Value: `["actor","other"]`, Type: "String", Version: 1},
	}}
	_, err := NewService(store, &fakeLock{}, nil, nil).Update(context.Background(), Actor{Subject: "actor"}, "admin-subs", UpdateInput{
		ValueType: featureflags.ValueJSON, Value: []any{"other"}, ExpectedVersion: 1,
	})
	if err != nil || store.parameters[Prefix+"admin-subs"].Value != `["other"]` {
		t.Fatalf("Update(admin-subs) error=%v value=%q", err, store.parameters[Prefix+"admin-subs"].Value)
	}
}

func TestUpdateAdminSubsRejectsEmptyArray(t *testing.T) {
	store := &fakeStore{parameters: map[string]featureflags.Parameter{
		Prefix + "admin-subs": {Name: Prefix + "admin-subs", Value: `["actor"]`, Type: "String", Version: 1},
	}}
	_, err := NewService(store, &fakeLock{}, nil, nil).Update(context.Background(), Actor{}, "admin-subs", UpdateInput{
		ValueType: featureflags.ValueJSON, Value: []any{}, ExpectedVersion: 1,
	})
	assertServiceError(t, err, "last_admin", 422)
}

func TestUpdateAdminSubsRejectsDuplicatesAndNonStrings(t *testing.T) {
	for _, value := range []any{[]any{"one", "one"}, []any{"one", 2}} {
		store := &fakeStore{parameters: map[string]featureflags.Parameter{
			Prefix + "admin-subs": {Name: Prefix + "admin-subs", Value: `["actor"]`, Type: "String", Version: 1},
		}}
		_, err := NewService(store, &fakeLock{}, nil, nil).Update(context.Background(), Actor{}, "admin-subs", UpdateInput{
			ValueType: featureflags.ValueJSON, Value: value, ExpectedVersion: 1,
		})
		assertServiceError(t, err, "invalid_value", 422)
	}
}

func TestServiceAuditsStructuredSuccessAndRejection(t *testing.T) {
	actor := Actor{Subject: "subject", Email: "person@example.com", PrincipalType: "spa", TraceID: "trace-123"}
	store := &fakeStore{parameters: map[string]featureflags.Parameter{
		Prefix + "flag": {Name: Prefix + "flag", Value: "false", Type: "String", Version: 7, Tags: map[string]string{"ManagedBy": "terraform"}},
	}}
	audit := &fakeAuditLogger{}
	service := NewService(store, &fakeLock{}, nil, audit)

	if _, err := service.Update(context.Background(), actor, "flag", UpdateInput{ValueType: featureflags.ValueBoolean, Value: true, ExpectedVersion: 7}); err != nil {
		t.Fatalf("Update() error = %v", err)
	}
	if _, err := service.Update(context.Background(), actor, "flag", UpdateInput{ValueType: featureflags.ValueBoolean, Value: false, ExpectedVersion: 7}); err == nil {
		t.Fatal("stale Update() unexpectedly succeeded")
	}
	if len(audit.events) != 2 {
		t.Fatalf("audit events = %#v", audit.events)
	}
	success := audit.events[0]
	if success.Actor != actor || success.Operation != "update" || success.Path != Prefix+"flag" || success.Ownership != OwnershipTerraform || success.OldValue == nil || success.NewValue == nil || success.OldValue.Value != false || success.NewValue.Value != true || success.ExpectedVersion != 7 || success.PreviousVersion != 7 || success.ResultVersion != 8 || success.Result != "success" || success.Reason != "" {
		t.Fatalf("success audit event = %#v", success)
	}
	rejected := audit.events[1]
	if rejected.Actor != actor || rejected.OldValue == nil || rejected.NewValue == nil || rejected.NewValue.Value != false || rejected.ExpectedVersion != 7 || rejected.PreviousVersion != 8 || rejected.Result != "rejected" || rejected.Reason != "version_conflict" {
		t.Fatalf("rejected audit event = %#v", rejected)
	}
}

func assertServiceError(t *testing.T, err error, code string, status int) *ServiceError {
	t.Helper()
	var serviceError *ServiceError
	if !errors.As(err, &serviceError) {
		t.Fatalf("error = %v, want ServiceError code %q", err, code)
	}
	if serviceError.Code != code || serviceError.HTTPStatus != status || serviceError.Message == "" {
		t.Fatalf("service error = %#v, want code=%q status=%d", serviceError, code, status)
	}
	return serviceError
}

func equalTags(got, want map[string]string) bool {
	if len(got) != len(want) {
		return false
	}
	for key, value := range want {
		if got[key] != value {
			return false
		}
	}
	return true
}

func TestServiceReturnsStableLeaseUnavailableError(t *testing.T) {
	lock := &fakeLock{err: ErrLeaseUnavailable}
	invalidator := &fakeInvalidator{}
	audit := &fakeAuditLogger{}
	service := NewService(&fakeStore{parameters: map[string]featureflags.Parameter{}}, lock, invalidator, audit)

	_, err := service.Create(context.Background(), Actor{}, CreateInput{Key: "flag", ValueType: featureflags.ValueBoolean, Value: true})
	assertServiceError(t, err, "lease_unavailable", 503)
	if invalidator.calls != 0 || len(audit.events) != 1 || audit.events[0].Reason != "lease_unavailable" {
		t.Fatalf("invalidations=%d audit=%#v", invalidator.calls, audit.events)
	}
}

func TestServiceCreateAdminSubsRejectsInvalidArrays(t *testing.T) {
	for _, testCase := range []struct {
		name      string
		valueType featureflags.ValueType
		value     any
		code      string
	}{
		{name: "non JSON type", valueType: featureflags.ValueString, value: `["admin"]`, code: "invalid_value"},
		{name: "empty", valueType: featureflags.ValueJSON, value: []any{}, code: "last_admin"},
		{name: "duplicate", valueType: featureflags.ValueJSON, value: []any{"admin", "admin"}, code: "invalid_value"},
		{name: "empty member", valueType: featureflags.ValueJSON, value: []any{""}, code: "invalid_value"},
		{name: "non string member", valueType: featureflags.ValueJSON, value: []any{"admin", 1}, code: "invalid_value"},
	} {
		t.Run(testCase.name, func(t *testing.T) {
			store := &fakeStore{parameters: map[string]featureflags.Parameter{}}
			invalidator := &fakeInvalidator{}
			_, err := NewService(store, &fakeLock{}, invalidator, nil).Create(context.Background(), Actor{}, CreateInput{
				Key: "admin-subs", ValueType: testCase.valueType, Value: testCase.value,
			})
			assertServiceError(t, err, testCase.code, 422)
			if len(store.parameters) != 0 || invalidator.calls != 0 {
				t.Fatalf("parameters=%#v invalidations=%d", store.parameters, invalidator.calls)
			}
		})
	}
}

func TestServiceUpdateReturnsVersionConflictBeforeInvalidValueValidation(t *testing.T) {
	store := &fakeStore{parameters: map[string]featureflags.Parameter{
		Prefix + "flag": {Name: Prefix + "flag", Value: "true", Type: "String", Version: 4},
	}}

	_, err := NewService(store, &fakeLock{}, nil, nil).Update(context.Background(), Actor{}, "flag", UpdateInput{
		ValueType: featureflags.ValueBoolean, Value: "not-a-bool", ExpectedVersion: 3,
	})
	serviceError := assertServiceError(t, err, "version_conflict", 409)
	if serviceError.Current == nil || serviceError.Current.Version != 4 {
		t.Fatalf("current = %#v", serviceError.Current)
	}
}

func TestServiceRejectsNilMutationLockWithoutStoreMutation(t *testing.T) {
	store := &fakeStore{parameters: map[string]featureflags.Parameter{}}
	audit := &fakeAuditLogger{}
	_, err := NewService(store, nil, nil, audit).Create(context.Background(), Actor{}, CreateInput{
		Key: "flag", ValueType: featureflags.ValueBoolean, Value: true,
	})
	assertServiceError(t, err, "mutation_lock_unconfigured", 500)
	if len(store.parameters) != 0 || len(audit.events) != 1 || audit.events[0].Reason != "mutation_lock_unconfigured" {
		t.Fatalf("parameters=%#v audit=%#v", store.parameters, audit.events)
	}
}

type contextAuditLogger struct {
	calls int
	event AuditEvent
	err   error
}

type contextFailingLock struct{}

func (contextFailingLock) WithLock(ctx context.Context, _ func(context.Context) error) error {
	return ctx.Err()
}

func (l *contextAuditLogger) Log(ctx context.Context, event AuditEvent) {
	l.calls++
	l.event = event
	l.err = ctx.Err()
}

func TestServiceAuditsExactlyOnceWithDetachedContextAfterCancellation(t *testing.T) {
	store := &fakeStore{parameters: map[string]featureflags.Parameter{}}
	audit := &contextAuditLogger{}
	ctx, cancel := context.WithCancel(context.Background())
	cancel()

	_, err := NewService(store, contextFailingLock{}, nil, audit).Create(ctx, Actor{TraceID: "trace"}, CreateInput{
		Key: "flag", ValueType: featureflags.ValueBoolean, Value: true,
	})
	if err == nil {
		t.Fatal("Create() unexpectedly succeeded")
	}
	if audit.calls != 1 || audit.err != nil || audit.event.Actor.TraceID != "trace" || audit.event.Result != "rejected" {
		t.Fatalf("audit calls=%d err=%v event=%#v", audit.calls, audit.err, audit.event)
	}
}

func TestServiceUpdateAuditsValidSubmittedValueOnStaleConflict(t *testing.T) {
	store := &fakeStore{parameters: map[string]featureflags.Parameter{
		Prefix + "flag": {Name: Prefix + "flag", Value: "true", Type: "String", Version: 4},
	}}
	audit := &fakeAuditLogger{}

	_, err := NewService(store, &fakeLock{}, nil, audit).Update(context.Background(), Actor{}, "flag", UpdateInput{
		ValueType: featureflags.ValueBoolean, Value: false, ExpectedVersion: 3,
	})
	assertServiceError(t, err, "version_conflict", 409)
	if len(audit.events) != 1 || audit.events[0].NewValue == nil || audit.events[0].NewValue.Value != false {
		t.Fatalf("audit events = %#v", audit.events)
	}
}

func TestServiceUpdateAuditsInvalidSubmittedValueAsVersionConflict(t *testing.T) {
	store := &fakeStore{parameters: map[string]featureflags.Parameter{
		Prefix + "flag": {Name: Prefix + "flag", Value: "true", Type: "String", Version: 4},
	}}
	audit := &fakeAuditLogger{}

	_, err := NewService(store, &fakeLock{}, nil, audit).Update(context.Background(), Actor{}, "flag", UpdateInput{
		ValueType: featureflags.ValueBoolean, Value: "not-a-bool", ExpectedVersion: 3,
	})
	assertServiceError(t, err, "version_conflict", 409)
	if len(audit.events) != 1 || audit.events[0].Reason != "version_conflict" || audit.events[0].NewValue != nil {
		t.Fatalf("audit events = %#v", audit.events)
	}
}

type contextBlockingStore struct {
	fakeStore
	updateStarted chan struct{}
	updateErr     error
}

func (s *contextBlockingStore) Update(ctx context.Context, path, value string) (featureflags.Parameter, error) {
	close(s.updateStarted)
	<-ctx.Done()
	s.updateErr = ctx.Err()
	return featureflags.Parameter{}, ctx.Err()
}

func TestServiceStopsStoreWorkWhenLeaseRenewalLosesOwnership(t *testing.T) {
	server := newLeaseServer(t, leaseObject{Metadata: leaseMetadata{ResourceVersion: "1"}})
	server.renewalLosesOwnership = true
	defer server.Close()
	lock := NewKubernetesLeaseLock(server.URL, "cyclops-cs", "cyclops-feature-flags-writer", "pod-a", time.Second, 10*time.Millisecond, time.Now)
	store := &contextBlockingStore{fakeStore: fakeStore{parameters: map[string]featureflags.Parameter{
		Prefix + "flag": {Name: Prefix + "flag", Value: "true", Type: "String", Version: 1},
	}}, updateStarted: make(chan struct{})}

	result := make(chan error, 1)
	go func() {
		_, err := NewService(store, lock, nil, nil).Update(context.Background(), Actor{}, "flag", UpdateInput{
			ValueType: featureflags.ValueBoolean, Value: false, ExpectedVersion: 1,
		})
		result <- err
	}()
	select {
	case <-store.updateStarted:
	case <-time.After(time.Second):
		t.Fatal("store update did not start")
	}
	select {
	case err := <-result:
		assertServiceError(t, err, "lease_unavailable", 503)
	case <-time.After(2 * time.Second):
		t.Fatal("service did not stop the store update after lease loss")
	}
	if !errors.Is(store.updateErr, context.Canceled) {
		t.Fatalf("store update error = %v, want context cancellation", store.updateErr)
	}
}

func TestServiceMapsMissingLeaseToStableError(t *testing.T) {
	service := NewService(&fakeStore{parameters: map[string]featureflags.Parameter{}}, &fakeLock{err: ErrLeaseNotFound}, nil, nil)
	_, err := service.Create(context.Background(), Actor{}, CreateInput{Key: "flag", ValueType: featureflags.ValueBoolean, Value: true})
	assertServiceError(t, err, "lease_not_found", 503)
}

func TestServiceNilStoreFailsClosed(t *testing.T) {
	service := NewService(nil, &fakeLock{}, nil, nil)
	if _, err := service.List(context.Background()); err == nil {
		t.Fatal("List() error = nil")
	} else {
		assertServiceError(t, err, "unsupported_provider", 501)
	}
	if _, err := service.Create(context.Background(), Actor{}, CreateInput{Key: "flag", ValueType: featureflags.ValueBoolean, Value: true}); err == nil {
		t.Fatal("Create() error = nil")
	} else {
		assertServiceError(t, err, "unsupported_provider", 501)
	}
}

func TestValidateKnownFlagRejectsInvalidUsagePrices(t *testing.T) {
	for _, typed := range []featureflags.TypedValue{
		{Type: featureflags.ValueString, Value: "0.1", Raw: "0.1"},
		{Type: featureflags.ValueNumber, Value: 0.0, Raw: "0"},
		{Type: featureflags.ValueNumber, Value: -1.0, Raw: "-1"},
	} {
		if err := validateKnownFlag("usage-vcpu-hour-price-usd", typed); err == nil {
			t.Fatalf("validateKnownFlag(%#v) error = nil", typed)
		}
	}
	if err := validateKnownFlag("usage-memory-gib-hour-price-usd", featureflags.TypedValue{Type: featureflags.ValueNumber, Value: 0.2, Raw: "0.2"}); err != nil {
		t.Fatalf("valid usage price: %v", err)
	}
}
