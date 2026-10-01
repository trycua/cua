package usage

import (
	"context"
	"fmt"
	"strings"
	"time"

	"github.com/jackc/pgx/v5/pgxpool"
)

const maxEvents = 10000

const usageSandboxEventsQuery = `select event_id, namespace, sandbox_name, sandbox_uid, pool_name,
       runtime, vm_name, event_type, observed_at
from k8s_reporting.usage_sandbox_events($1, $2, $3)`

type PostgresEventStore struct {
	pool *pgxpool.Pool
}

func NewPostgresEventStore(ctx context.Context, databaseURL string) (*PostgresEventStore, error) {
	config, err := pgxpool.ParseConfig(databaseURL)
	if err != nil {
		return nil, fmt.Errorf("parse usage database URL: %w", err)
	}
	if config.ConnConfig.User != "cyclops_usage_reader" {
		return nil, fmt.Errorf("usage database URL must use cyclops_usage_reader")
	}
	config.MaxConns = 4
	config.MinConns = 0
	config.MaxConnIdleTime = time.Minute
	config.MaxConnLifetime = 15 * time.Minute
	pool, err := pgxpool.NewWithConfig(ctx, config)
	if err != nil {
		return nil, fmt.Errorf("open usage database pool: %w", err)
	}
	return &PostgresEventStore{pool: pool}, nil
}

func (s *PostgresEventStore) Close() {
	if s != nil && s.pool != nil {
		s.pool.Close()
	}
}

func (s *PostgresEventStore) Events(ctx context.Context, tenant string, start, end time.Time) ([]SandboxEvent, error) {
	if strings.TrimSpace(tenant) == "" || start.IsZero() || end.IsZero() || !end.After(start) {
		return nil, fmt.Errorf("invalid usage event query")
	}
	rows, err := s.pool.Query(ctx, usageSandboxEventsQuery, tenant, start, end)
	if err != nil {
		return nil, fmt.Errorf("query usage sandbox events: %w", err)
	}
	defer rows.Close()

	events := make([]SandboxEvent, 0)
	for rows.Next() {
		if len(events) >= maxEvents {
			return nil, fmt.Errorf("usage event result exceeds %d events", maxEvents)
		}
		var event SandboxEvent
		if err := rows.Scan(&event.EventID, &event.Namespace, &event.SandboxName, &event.SandboxUID, &event.PoolName, &event.Runtime, &event.VMName, &event.EventType, &event.ObservedAt); err != nil {
			return nil, fmt.Errorf("scan usage sandbox event: %w", err)
		}
		if err := validateSandboxEvent(event); err != nil {
			return nil, err
		}
		events = append(events, event)
	}
	if err := rows.Err(); err != nil {
		return nil, fmt.Errorf("read usage sandbox events: %w", err)
	}
	return events, nil
}

func validateSandboxEvent(event SandboxEvent) error {
	fields := []string{event.Namespace, event.SandboxName, event.SandboxUID, event.PoolName, event.Runtime, event.VMName, event.EventType}
	if event.EventID == [16]byte{} || event.ObservedAt.IsZero() {
		return fmt.Errorf("usage sandbox event contains empty required fields")
	}
	for _, field := range fields {
		if field == "" || field != strings.TrimSpace(field) {
			return fmt.Errorf("usage sandbox event contains unnormalized required fields")
		}
	}
	if len(event.Namespace) > 63 || !dns1123Label.MatchString(event.Namespace) {
		return fmt.Errorf("usage sandbox event contains invalid namespace")
	}
	if !validPoolName(event.PoolName) {
		return fmt.Errorf("usage sandbox event contains invalid pool name")
	}
	return nil
}

var _ EventStore = (*PostgresEventStore)(nil)

const reservationFactsQuery = `select namespace, sandbox_uid, sandbox_name, pool_name, runtime,
       hour_start, hour_end, virtual_cpu_core_seconds, virtual_memory_byte_seconds,
       ready_seconds, covered_seconds
from k8s_reporting.reservation_hour_facts($1, $2, $3)`

const reservationStatusQuery = `select data_as_of, complete
from k8s_reporting.reservation_meter_status($1, $2, $3)`

func (s *PostgresEventStore) Reservations(ctx context.Context, tenant, cluster string, start, end time.Time) ([]ReservationFact, time.Time, bool, error) {
	if strings.TrimSpace(tenant) == "" || strings.TrimSpace(cluster) == "" || start.IsZero() || end.IsZero() || !end.After(start) {
		return nil, time.Time{}, false, fmt.Errorf("invalid reservation query")
	}
	var asOf time.Time
	var complete bool
	if err := s.pool.QueryRow(ctx, reservationStatusQuery, cluster, start, end).Scan(&asOf, &complete); err != nil {
		return nil, time.Time{}, false, fmt.Errorf("query reservation meter status: %w", err)
	}
	rows, err := s.pool.Query(ctx, reservationFactsQuery, tenant, start, end)
	if err != nil {
		return nil, time.Time{}, false, fmt.Errorf("query reservation facts: %w", err)
	}
	defer rows.Close()
	facts := make([]ReservationFact, 0)
	for rows.Next() {
		var fact ReservationFact
		if err := rows.Scan(&fact.Namespace, &fact.SandboxUID, &fact.SandboxName, &fact.PoolName, &fact.Runtime, &fact.HourStart, &fact.HourEnd, &fact.VirtualCPUCoreSeconds, &fact.VirtualMemoryByteSeconds, &fact.ReadySeconds, &fact.CoveredSeconds); err != nil {
			return nil, time.Time{}, false, fmt.Errorf("scan reservation fact: %w", err)
		}
		facts = append(facts, fact)
	}
	if err := rows.Err(); err != nil {
		return nil, time.Time{}, false, fmt.Errorf("read reservation facts: %w", err)
	}
	return facts, asOf.UTC(), !complete, nil
}

var _ ReservationStore = (*PostgresEventStore)(nil)
