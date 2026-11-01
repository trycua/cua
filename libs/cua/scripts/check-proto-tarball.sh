#!/usr/bin/env bash
# Check the cua-proto-<version>.tar.gz release asset the way a user of
# another language uses it: unpack it, `buf build` it, `buf generate` the Go
# client with its own buf.gen.yaml, and compile a program against the
# generated code. With SPACESD_ADDR (host:port, plaintext) and SPACESD_TOKEN
# set, the program also calls SystemService.Health and GetCapabilities on
# that spacesd.
#
# Usage: libs/cua/scripts/check-proto-tarball.sh [<work-dir>]
# Needs: buf (network for the buf.build remote plugins), go.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
version="$(tr -d '[:space:]' < "$here/../VERSION")"
work="${1:-$(mktemp -d)}"
mkdir -p "$work"
work="$(cd "$work" && pwd)"

tarball="$("$here/package-proto.sh" "$version" "$work/dist")"
rm -rf "$work/cua-proto-$version" "$work/app"
tar -xzf "$tarball" -C "$work"
src="$work/cua-proto-$version"
test -f "$src/README.md" && test -f "$src/buf.gen.yaml"
(cd "$src" && buf build -o /dev/null)

module="example.com/cuaharness"
sed -i.bak "s#example.com/yourapp/gen/go#$module/gen/go#" "$src/buf.gen.yaml"
(cd "$src" && buf generate)

mkdir -p "$work/app"
mv "$src/gen" "$work/app/gen"
cat > "$work/app/main.go" <<'GO'
// A client of a Space's spacesd built only from cua-proto-<version>.tar.gz.
package main

import (
	"context"
	"fmt"
	"log"
	"os"
	"time"

	envv1 "example.com/cuaharness/gen/go/cua/env/v1"
	"google.golang.org/grpc"
	"google.golang.org/grpc/credentials/insecure"
	"google.golang.org/grpc/metadata"
)

func main() {
	addr := os.Getenv("SPACESD_ADDR")
	if addr == "" {
		fmt.Println("compiled; set SPACESD_ADDR and SPACESD_TOKEN to call a Space")
		return
	}
	conn, err := grpc.NewClient(addr, grpc.WithTransportCredentials(insecure.NewCredentials()))
	if err != nil {
		log.Fatal(err)
	}
	defer conn.Close()
	ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
	defer cancel()
	ctx = metadata.AppendToOutgoingContext(ctx, "authorization", "Bearer "+os.Getenv("SPACESD_TOKEN"))
	system := envv1.NewSystemServiceClient(conn)
	health, err := system.Health(ctx, &envv1.HealthRequest{})
	if err != nil {
		log.Fatal(err)
	}
	fmt.Printf("Health: %s, uptime %s\n", health.GetStatus(), health.GetUptime().AsDuration())
	for _, c := range health.GetComponents() {
		fmt.Printf("  %s: %s\n", c.GetName(), c.GetStatus())
	}
	caps, err := system.GetCapabilities(ctx, &envv1.GetCapabilitiesRequest{})
	if err != nil {
		log.Fatal(err)
	}
	fmt.Printf("GetCapabilities: protocol v%d rev %d, %s\n", caps.GetProtocolVersion(), caps.GetProtocolRevision(), caps.GetOs().GetPrettyName())
}
GO
cd "$work/app"
go mod init "$module" >/dev/null 2>&1
go mod tidy >/dev/null
go vet ./...
go build -o cua-proto-go-check .
./cua-proto-go-check
echo "ok: cua-proto-$version.tar.gz builds with buf and generates a compiling Go client ($work)"
