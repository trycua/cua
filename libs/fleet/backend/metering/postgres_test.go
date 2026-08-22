package metering

import (
	"reflect"
	"regexp"
	"sort"
	"strings"
	"testing"
)

func TestReservationHourCompletionStatementsRetainParameters(t *testing.T) {
	if strings.Count(lockReservationHourCompletionStatement, "$1") != 1 {
		t.Fatalf("hour completion lock statement = %q", lockReservationHourCompletionStatement)
	}
	if strings.Count(selectReservationHourCompletionStatement, "$1") != 1 {
		t.Fatalf("hour completion select statement = %q", selectReservationHourCompletionStatement)
	}
	parameters := regexp.MustCompile(`\$([0-9]+)`).FindAllString(insertReservationHourCompletionStatement, -1)
	sort.Strings(parameters)
	want := []string{"$1", "$10", "$11", "$12", "$2", "$3", "$4", "$5", "$6", "$7", "$8", "$9"}
	if !reflect.DeepEqual(parameters, want) {
		t.Fatalf("hour completion insert parameters = %v, want %v", parameters, want)
	}
}
