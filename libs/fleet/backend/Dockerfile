# Cyclops-CS backend — Go sidecar exposed inside the cyclops-cs Pod via
# nginx (cyclops-cs/nginx.conf routes /api/{keys,gateway} to 127.0.0.1:8080).
#
# Build context is the repo root so the pkg/featureflags module (referenced
# by cyclops-cs/backend/go.mod via a local `replace`) is reachable. The
# build-cyclops-cs.yml workflow sets the context to `.` for this image.
FROM --platform=linux/amd64 golang:1.25-alpine AS builder
WORKDIR /src
COPY pkg/featureflags ./pkg/featureflags
COPY cyclops-cs/backend ./cyclops-cs/backend
WORKDIR /src/cyclops-cs/backend
RUN go mod download
# Ensure go.mod/go.sum are tidy (catches manual edits without `go mod tidy`).
RUN go mod tidy
# Regenerate docs/ in CI so the embedded swagger spec stays in sync with
# annotated source. swag is installed via `go install` rather than added
# as a runtime dep — we only need its CLI here.
RUN go install github.com/swaggo/swag/cmd/swag@v1.16.4 && \
    /go/bin/swag init -g main.go -o ./docs --parseDependency --parseInternal
RUN CGO_ENABLED=0 GOOS=linux go build -trimpath -ldflags="-s -w" -o /out/cyclops-cs-backend .

# Distroless static — Rego policy is embedded into the binary via
# //go:embed in auth/middlewares.go, so no extra file is shipped here.
FROM gcr.io/distroless/static:nonroot AS runtime
WORKDIR /app
COPY --from=builder /out/cyclops-cs-backend /app/cyclops-cs-backend
USER nonroot:nonroot
EXPOSE 8080
ENTRYPOINT ["/app/cyclops-cs-backend"]
