package metering

import (
	"fmt"
	"math"
	"sort"
	"time"
)

type Sample struct {
	Timestamp time.Time
	Value     float64
}

type Integral struct {
	ValueSeconds   float64
	PresentSeconds float64
}

func Integrate(samples []Sample, start, end time.Time, maxValidity time.Duration) (Integral, error) {
	if start.IsZero() || end.IsZero() || !end.After(start) || maxValidity <= 0 {
		return Integral{}, fmt.Errorf("invalid integration window")
	}

	ordered := append([]Sample(nil), samples...)
	sort.Slice(ordered, func(i, j int) bool { return ordered[i].Timestamp.Before(ordered[j].Timestamp) })

	var result Integral
	for index, sample := range ordered {
		if sample.Timestamp.IsZero() || math.IsNaN(sample.Value) || math.IsInf(sample.Value, 0) {
			return Integral{}, fmt.Errorf("invalid metric sample")
		}
		if index > 0 && !sample.Timestamp.After(ordered[index-1].Timestamp) {
			return Integral{}, fmt.Errorf("metric samples must have unique timestamps")
		}

		intervalStart := sample.Timestamp
		if intervalStart.Before(start) {
			intervalStart = start
		}
		intervalEnd := sample.Timestamp.Add(maxValidity)
		if index+1 < len(ordered) && ordered[index+1].Timestamp.Before(intervalEnd) {
			intervalEnd = ordered[index+1].Timestamp
		}
		if intervalEnd.After(end) {
			intervalEnd = end
		}
		if !intervalEnd.After(intervalStart) || !intervalStart.Before(end) {
			continue
		}

		seconds := intervalEnd.Sub(intervalStart).Seconds()
		result.ValueSeconds += sample.Value * seconds
		result.PresentSeconds += seconds
	}
	return result, nil
}
