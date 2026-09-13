package featureflagadmin

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"math"
	"regexp"
	"sort"
	"strconv"
	"strings"
	"time"

	"github.com/trycua/cloud/pkg/featureflags"
)

const Prefix = "/feature-flags/cyclops-cs/"

var keyPattern = regexp.MustCompile(`^[a-z0-9]([-a-z0-9]*[a-z0-9])?$`)

type Ownership string

const (
	OwnershipTerraform Ownership = "terraform"
	OwnershipAdHoc     Ownership = "ad_hoc"
	OwnershipExternal  Ownership = "external"
)

type Flag struct {
	Key         string                 `json:"key"`
	Path        string                 `json:"path"`
	ValueType   featureflags.ValueType `json:"value_type"`
	Value       any                    `json:"value"`
	RawValue    string                 `json:"raw_value"`
	Ownership   Ownership              `json:"ownership"`
	ManagedBy   string                 `json:"managed_by,omitempty"`
	Deletable   bool                   `json:"deletable"`
	Version     int64                  `json:"version"`
	ModifiedAt  time.Time              `json:"modified_at"`
	Description string                 `json:"description,omitempty"`
	Tags        map[string]string      `json:"tags"`
}

type Actor struct {
	Subject       string
	Email         string
	PrincipalType string
	TraceID       string
}

type CreateInput struct {
	Key         string
	ValueType   featureflags.ValueType
	Value       any
	Description string
}

type UpdateInput struct {
	ValueType       featureflags.ValueType
	Value           any
	ExpectedVersion int64
}

type AuditEvent struct {
	Event           string
	Timestamp       time.Time
	Actor           Actor
	Operation       string
	Key             string
	Path            string
	Ownership       Ownership
	OldValue        *featureflags.TypedValue
	NewValue        *featureflags.TypedValue
	ExpectedVersion int64
	PreviousVersion int64
	ResultVersion   int64
	Result          string
	Reason          string
}

type MutationLock interface {
	WithLock(context.Context, func(context.Context) error) error
}

type CacheInvalidator interface {
	InvalidateFeatureFlags()
}

type AuditLogger interface {
	Log(context.Context, AuditEvent)
}

type ServiceError struct {
	Code       string
	HTTPStatus int
	Message    string
	Current    *Flag
	Cause      error
}

func (e *ServiceError) Error() string { return e.Message }

func (e *ServiceError) Unwrap() error { return e.Cause }

type Service struct {
	store       featureflags.ManagementStore
	lock        MutationLock
	invalidator CacheInvalidator
	audit       AuditLogger
}

func NewService(store featureflags.ManagementStore, lock MutationLock, invalidator CacheInvalidator, audit AuditLogger) *Service {
	if store == nil {
		store = unsupportedStore{}
	}
	return &Service{store: store, lock: lock, invalidator: invalidator, audit: audit}
}

func (s *Service) List(ctx context.Context) ([]Flag, error) {
	parameters, err := s.store.List(ctx, Prefix)
	if err != nil {
		return nil, errors.Join(serviceStoreError(err), err)
	}
	flags := make([]Flag, 0, len(parameters))
	for _, parameter := range parameters {
		if parameter.Type == "SecureString" {
			continue
		}
		key, ok := keyFromPath(parameter.Name)
		if !ok {
			continue
		}
		flag, err := flagFromParameter(key, parameter)
		if err != nil {
			return nil, err
		}
		flags = append(flags, flag)
	}
	sort.Slice(flags, func(i, j int) bool { return flags[i].Key < flags[j].Key })
	return flags, nil
}

func (s *Service) Create(ctx context.Context, actor Actor, input CreateInput) (flag Flag, err error) {
	path := Prefix + input.Key
	event := AuditEvent{Event: "feature_flag_admin", Timestamp: time.Now().UTC(), Actor: actor, Operation: "create", Key: input.Key, Path: path, Ownership: OwnershipAdHoc, Result: "rejected"}
	defer func() { s.logAudit(ctx, event, err) }()

	err = s.withLock(ctx, func(lockCtx context.Context) error {
		if err := validateKey(input.Key); err != nil {
			return err
		}
		typed, err := featureflags.SerializeTypedValue(input.ValueType, input.Value)
		if err != nil {
			return errors.Join(invalidValueError(err), err)
		}
		event.NewValue = &typed
		if err := validateKnownFlag(input.Key, typed); err != nil {
			return err
		}
		parameter, err := s.store.Create(lockCtx, featureflags.Parameter{
			Name: path, Value: typed.Raw, Type: "String", Description: input.Description,
			Tags: map[string]string{"ManagedBy": "cyclops-cs-admin", "Feature": "cyclops-cs", "Environment": "production"},
		})
		if err != nil {
			return errors.Join(serviceStoreError(err), err)
		}
		flag, err = flagFromParameter(input.Key, parameter)
		if err != nil {
			return err
		}
		event.ResultVersion = flag.Version
		event.Result = "success"
		if s.invalidator != nil {
			s.invalidator.InvalidateFeatureFlags()
		}
		return nil
	})
	return flag, err
}

func (s *Service) Update(ctx context.Context, actor Actor, key string, input UpdateInput) (flag Flag, err error) {
	path := Prefix + key
	event := AuditEvent{Event: "feature_flag_admin", Timestamp: time.Now().UTC(), Actor: actor, Operation: "update", Key: key, Path: path, ExpectedVersion: input.ExpectedVersion, Result: "rejected"}
	defer func() { s.logAudit(ctx, event, err) }()

	err = s.withLock(ctx, func(lockCtx context.Context) error {
		if err := validateKey(key); err != nil {
			return err
		}
		parameter, err := s.store.Get(lockCtx, path)
		if err != nil {
			return errors.Join(serviceStoreError(err), err)
		}
		current, err := flagFromParameter(key, parameter)
		if err != nil {
			return err
		}
		event.Ownership = current.Ownership
		event.PreviousVersion = current.Version
		old := featureflags.TypedValue{Type: current.ValueType, Value: current.Value, Raw: current.RawValue}
		event.OldValue = &old
		if current.Version != input.ExpectedVersion {
			typed, serializeErr := featureflags.SerializeTypedValue(input.ValueType, input.Value)
			if serializeErr != nil {
				return errors.Join(&ServiceError{Code: "version_conflict", HTTPStatus: 409, Message: "feature flag version is stale", Current: &current, Cause: serializeErr}, serializeErr)
			}
			event.NewValue = &typed
			return &ServiceError{Code: "version_conflict", HTTPStatus: 409, Message: "feature flag version is stale", Current: &current}
		}
		typed, err := featureflags.SerializeTypedValue(input.ValueType, input.Value)
		if err != nil {
			return errors.Join(invalidValueError(err), err)
		}
		event.NewValue = &typed
		if err := validateKnownFlag(key, typed); err != nil {
			return err
		}
		parameter, err = s.store.Update(lockCtx, path, typed.Raw)
		if err != nil {
			return errors.Join(serviceStoreError(err), err)
		}
		flag, err = flagFromParameter(key, parameter)
		if err != nil {
			return err
		}
		event.ResultVersion = flag.Version
		event.Result = "success"
		if s.invalidator != nil {
			s.invalidator.InvalidateFeatureFlags()
		}
		return nil
	})
	return flag, err
}

func (s *Service) Delete(ctx context.Context, actor Actor, key string, expectedVersion int64) (err error) {
	path := Prefix + key
	event := AuditEvent{Event: "feature_flag_admin", Timestamp: time.Now().UTC(), Actor: actor, Operation: "delete", Key: key, Path: path, ExpectedVersion: expectedVersion, Result: "rejected"}
	defer func() { s.logAudit(ctx, event, err) }()

	err = s.withLock(ctx, func(lockCtx context.Context) error {
		if err := validateKey(key); err != nil {
			return err
		}
		if key == "admin-subs" {
			return &ServiceError{Code: "last_admin", HTTPStatus: 422, Message: "admin-subs cannot be deleted"}
		}
		parameter, err := s.store.Get(lockCtx, path)
		if err != nil {
			return errors.Join(serviceStoreError(err), err)
		}
		current, err := flagFromParameter(key, parameter)
		if err != nil {
			return err
		}
		event.Ownership = current.Ownership
		event.PreviousVersion = current.Version
		old := featureflags.TypedValue{Type: current.ValueType, Value: current.Value, Raw: current.RawValue}
		event.OldValue = &old
		if current.Version != expectedVersion {
			return &ServiceError{Code: "version_conflict", HTTPStatus: 409, Message: "feature flag version is stale", Current: &current}
		}
		if !current.Deletable {
			code := "unknown_ownership"
			message := "feature flag ownership is unknown and does not permit deletion"
			if current.Ownership == OwnershipTerraform {
				code = "managed_flag"
				message = "Terraform-managed feature flags cannot be deleted"
			}
			return &ServiceError{Code: code, HTTPStatus: 422, Message: message, Current: &current}
		}
		if err := s.store.Delete(lockCtx, path); err != nil {
			return errors.Join(serviceStoreError(err), err)
		}
		event.Result = "success"
		if s.invalidator != nil {
			s.invalidator.InvalidateFeatureFlags()
		}
		return nil
	})
	return err
}

func (s *Service) withLock(ctx context.Context, callback func(context.Context) error) error {
	if s.lock == nil {
		return &ServiceError{Code: "mutation_lock_unconfigured", HTTPStatus: 500, Message: "feature flag mutation lock is not configured"}
	}
	if err := s.lock.WithLock(ctx, callback); err != nil {
		if errors.Is(err, ErrLeaseNotFound) {
			return errors.Join(&ServiceError{Code: "lease_not_found", HTTPStatus: 503, Message: "feature flag mutation lease was not found", Cause: err}, err)
		}
		if errors.Is(err, ErrLeaseUnavailable) {
			return errors.Join(&ServiceError{Code: "lease_unavailable", HTTPStatus: 503, Message: "feature flag mutation lock is unavailable", Cause: err}, err)
		}
		return err
	}
	return nil
}

func (s *Service) logAudit(_ context.Context, event AuditEvent, err error) {
	if s.audit == nil {
		return
	}
	if err != nil {
		event.Result = "rejected"
		var serviceError *ServiceError
		if errors.As(err, &serviceError) {
			event.Reason = serviceError.Code
		} else {
			event.Reason = "internal_error"
		}
	}
	auditCtx, cancel := context.WithTimeout(context.Background(), time.Second)
	defer cancel()
	s.audit.Log(auditCtx, event)
}

func flagFromParameter(key string, parameter featureflags.Parameter) (Flag, error) {
	if parameter.Type == "SecureString" {
		return Flag{}, &ServiceError{Code: "unsupported_parameter", HTTPStatus: 422, Message: "SecureString feature flags are unsupported"}
	}
	typed, err := featureflags.InferTypedValue(parameter.Value)
	if err != nil {
		return Flag{}, fmt.Errorf("infer feature flag value: %w", err)
	}
	managedBy := parameter.Tags["ManagedBy"]
	ownership := ownershipFromManagedBy(managedBy)
	return Flag{
		Key: key, Path: parameter.Name, ValueType: typed.Type, Value: typed.Value, RawValue: typed.Raw,
		Ownership: ownership, ManagedBy: managedBy, Deletable: ownership == OwnershipAdHoc,
		Version: parameter.Version, ModifiedAt: parameter.LastModified, Description: parameter.Description, Tags: cloneTags(parameter.Tags),
	}, nil
}

func ownershipFromManagedBy(managedBy string) Ownership {
	switch managedBy {
	case "terraform":
		return OwnershipTerraform
	case "cyclops-cs-admin":
		return OwnershipAdHoc
	default:
		return OwnershipExternal
	}
}

func keyFromPath(path string) (string, bool) {
	if !strings.HasPrefix(path, Prefix) {
		return "", false
	}
	key := strings.TrimPrefix(path, Prefix)
	if validateKey(key) != nil {
		return "", false
	}
	return key, true
}

func validateKey(key string) error {
	if len(key) == 0 || len(key) > 63 || !keyPattern.MatchString(key) {
		return &ServiceError{Code: "invalid_key", HTTPStatus: 400, Message: "feature flag key must be a lowercase DNS label"}
	}
	return nil
}

func validateKnownFlag(key string, typed featureflags.TypedValue) error {
	switch key {
	case "admin-subs":
		return validateAdminSubs(typed)
	case "usage-vcpu-hour-price-usd", "usage-memory-gib-hour-price-usd":
		return validateUsagePrice(typed)
	default:
		return nil
	}
}

func validateUsagePrice(typed featureflags.TypedValue) error {
	if typed.Type != featureflags.ValueNumber {
		return &ServiceError{Code: "invalid_value", HTTPStatus: 422, Message: "usage prices must be positive numbers"}
	}
	value, err := strconv.ParseFloat(typed.Raw, 64)
	if err != nil {
		return errors.Join(&ServiceError{Code: "invalid_value", HTTPStatus: 422, Message: "usage prices must be positive numbers", Cause: err}, err)
	}
	if math.IsNaN(value) || math.IsInf(value, 0) || value <= 0 {
		return &ServiceError{Code: "invalid_value", HTTPStatus: 422, Message: "usage prices must be positive numbers"}
	}
	return nil
}

func validateAdminSubs(typed featureflags.TypedValue) error {
	if typed.Type != featureflags.ValueJSON {
		return &ServiceError{Code: "invalid_value", HTTPStatus: 422, Message: "admin-subs must be a JSON string array"}
	}
	var values []any
	if err := json.Unmarshal([]byte(typed.Raw), &values); err != nil {
		return errors.Join(&ServiceError{Code: "invalid_value", HTTPStatus: 422, Message: "admin-subs must be a JSON string array", Cause: err}, err)
	}
	if len(values) == 0 {
		return &ServiceError{Code: "last_admin", HTTPStatus: 422, Message: "admin-subs must contain at least one administrator"}
	}
	seen := make(map[string]struct{}, len(values))
	for _, value := range values {
		subject, ok := value.(string)
		if !ok || subject == "" {
			return &ServiceError{Code: "invalid_value", HTTPStatus: 422, Message: "admin-subs must contain non-empty strings"}
		}
		if _, duplicate := seen[subject]; duplicate {
			return &ServiceError{Code: "invalid_value", HTTPStatus: 422, Message: "admin-subs must not contain duplicate subjects"}
		}
		seen[subject] = struct{}{}
	}
	return nil
}

func invalidValueError(err error) error {
	return &ServiceError{Code: "invalid_value", HTTPStatus: 400, Message: fmt.Sprintf("invalid feature flag value: %v", err), Cause: err}
}

func serviceStoreError(err error) error {
	if errors.Is(err, featureflags.ErrSecureString) {
		return &ServiceError{Code: "unsupported_parameter", HTTPStatus: 422, Message: "SecureString feature flags are unsupported", Cause: err}
	}
	switch {
	case errors.Is(err, featureflags.ErrUnsupportedProvider):
		return &ServiceError{Code: "unsupported_provider", HTTPStatus: 501, Message: "feature flag management provider is unsupported", Cause: err}
	case errors.Is(err, featureflags.ErrParameterExists):
		return &ServiceError{Code: "flag_exists", HTTPStatus: 409, Message: "feature flag already exists", Cause: err}
	case errors.Is(err, featureflags.ErrParameterNotFound):
		return &ServiceError{Code: "flag_not_found", HTTPStatus: 404, Message: "feature flag not found", Cause: err}
	case errors.Is(err, featureflags.ErrAccessDenied):
		return &ServiceError{Code: "access_denied", HTTPStatus: 403, Message: "feature flag store access denied", Cause: err}
	default:
		return &ServiceError{Code: "store_error", HTTPStatus: 502, Message: "feature flag store operation failed", Cause: err}
	}
}

func cloneTags(tags map[string]string) map[string]string {
	if tags == nil {
		return map[string]string{}
	}
	cloned := make(map[string]string, len(tags))
	for key, value := range tags {
		cloned[key] = value
	}
	return cloned
}

type unsupportedStore struct{}

func (unsupportedStore) List(context.Context, string) ([]featureflags.Parameter, error) {
	return nil, featureflags.ErrUnsupportedProvider
}
func (unsupportedStore) Get(context.Context, string) (featureflags.Parameter, error) {
	return featureflags.Parameter{}, featureflags.ErrUnsupportedProvider
}
func (unsupportedStore) Create(context.Context, featureflags.Parameter) (featureflags.Parameter, error) {
	return featureflags.Parameter{}, featureflags.ErrUnsupportedProvider
}
func (unsupportedStore) Update(context.Context, string, string) (featureflags.Parameter, error) {
	return featureflags.Parameter{}, featureflags.ErrUnsupportedProvider
}
func (unsupportedStore) Delete(context.Context, string) error {
	return featureflags.ErrUnsupportedProvider
}

type noOpMutationLock struct{}

func (noOpMutationLock) WithLock(ctx context.Context, callback func(context.Context) error) error {
	return callback(ctx)
}
func NewUnsupportedService(audit AuditLogger) *Service {
	return NewService(unsupportedStore{}, noOpMutationLock{}, nil, audit)
}
