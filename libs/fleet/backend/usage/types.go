package usage

import (
	"context"
	"time"

	"github.com/google/uuid"
)

type Timeframe string
type Interval string

const (
	Timeframe24H Timeframe = "24h"
	Timeframe7D  Timeframe = "7d"
	Timeframe30D Timeframe = "30d"

	IntervalHour Interval = "1h"
	IntervalDay  Interval = "1d"
)

type Query struct {
	ActorSubject string
	Subject      string
	Admin        bool
	Timeframe    Timeframe
}

type PoolQuery struct {
	Query
	PoolID   string
	Interval Interval
}

type MetricTotals struct {
	Consumed    float64 `json:"consumed"`
	Provisioned float64 `json:"provisioned"`
}

type PoolSummary struct {
	ID      string       `json:"id"`
	Name    string       `json:"name"`
	CPU     MetricTotals `json:"cpu"`
	Memory  MetricTotals `json:"memory"`
	CostUSD float64      `json:"cost_usd"`
}

type OverviewResponse struct {
	DataAsOf time.Time     `json:"data_as_of"`
	Partial  bool          `json:"partial"`
	Pools    []PoolSummary `json:"pools"`
}

type PoolIdentity struct {
	ID   string `json:"id"`
	Name string `json:"name"`
}

type Bucket struct {
	Start             time.Time `json:"start"`
	End               time.Time `json:"end"`
	CPUConsumed       float64   `json:"cpu_consumed"`
	CPUProvisioned    float64   `json:"cpu_provisioned"`
	MemoryConsumed    float64   `json:"memory_consumed"`
	MemoryProvisioned float64   `json:"memory_provisioned"`
}

type PoolDetailResponse struct {
	DataAsOf time.Time    `json:"data_as_of"`
	Partial  bool         `json:"partial"`
	Pool     PoolIdentity `json:"pool"`
	Buckets  []Bucket     `json:"buckets"`
}

type Provider interface {
	Overview(context.Context, Query) (OverviewResponse, error)
	PoolDetail(context.Context, PoolQuery) (PoolDetailResponse, error)
}

type SandboxEvent struct {
	EventID     uuid.UUID
	Namespace   string
	SandboxName string
	SandboxUID  string
	PoolName    string
	Runtime     string
	VMName      string
	EventType   string
	ObservedAt  time.Time
}

type EventStore interface {
	// Events returns database event order; equal observed timestamps remain partial.
	Events(context.Context, string, time.Time, time.Time) ([]SandboxEvent, error)
}

type Allocation struct {
	Start                  time.Time
	End                    time.Time
	Namespace              string
	Pod                    string
	Minutes                float64
	CPUUsageAverage        float64
	CPURequestAverage      float64
	RAMUsageAverageBytes   float64
	RAMRequestAverageBytes float64
	CostUSD                float64
}

type AllocationClient interface {
	Allocations(context.Context, time.Time, time.Time, time.Duration, []string) ([]Allocation, time.Time, bool, error)
}

const gibibyte = float64(1 << 30)

type ReservationFact struct {
	Namespace                string
	SandboxUID               string
	SandboxName              string
	PoolName                 string
	Runtime                  string
	HourStart                time.Time
	HourEnd                  time.Time
	VirtualCPUCoreSeconds    float64
	VirtualMemoryByteSeconds float64
	ReadySeconds             float64
	CoveredSeconds           float64
}

type ReservationStore interface {
	Reservations(context.Context, string, string, time.Time, time.Time) ([]ReservationFact, time.Time, bool, error)
}
