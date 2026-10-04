package metering

import (
	"math"
	"testing"
	"time"
)

func TestIntegrateUsesPreviousSampleAtWindowBoundary(t *testing.T) {
	start := time.Date(2026, 8, 22, 7, 0, 0, 0, time.UTC)
	result, err := Integrate([]Sample{
		{Timestamp: start.Add(-15 * time.Second), Value: 4},
		{Timestamp: start.Add(15 * time.Second), Value: 4},
	}, start, start.Add(30*time.Second), time.Minute)
	if err != nil {
		t.Fatal(err)
	}
	if result.ValueSeconds != 120 || result.PresentSeconds != 30 {
		t.Fatalf("integral = %+v, want 120 value-seconds over 30 seconds", result)
	}
}

func TestIntegrateCapsStaleSamples(t *testing.T) {
	start := time.Date(2026, 8, 22, 7, 0, 0, 0, time.UTC)
	result, err := Integrate([]Sample{{Timestamp: start, Value: 8}}, start, start.Add(time.Hour), time.Minute)
	if err != nil {
		t.Fatal(err)
	}
	if result.ValueSeconds != 480 || result.PresentSeconds != 60 {
		t.Fatalf("integral = %+v, want one minute of validity", result)
	}
}

func TestIntegrateRejectsNonFiniteSamples(t *testing.T) {
	start := time.Date(2026, 8, 22, 7, 0, 0, 0, time.UTC)
	_, err := Integrate([]Sample{{Timestamp: start, Value: math.NaN()}}, start, start.Add(time.Hour), time.Minute)
	if err == nil {
		t.Fatal("expected non-finite sample rejection")
	}
}
