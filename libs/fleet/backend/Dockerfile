# Cyclops-CS backend and Kubernetes state projector.
# Build context is the repository root.
FROM --platform=linux/amd64 golang:1.24-bookworm AS go-builder
WORKDIR /src
COPY pkg/featureflags ./pkg/featureflags
COPY cyclops-cs/backend ./cyclops-cs/backend
WORKDIR /src/cyclops-cs/backend
RUN go mod download
RUN go mod tidy
RUN go install github.com/swaggo/swag/cmd/swag@v1.16.4 && \
    /go/bin/swag init -g main.go -o ./docs --parseDependency --parseInternal
RUN CGO_ENABLED=1 GOOS=linux go build -trimpath -ldflags="-s -w" -o /out/cyclops-cs-backend . \
    && CGO_ENABLED=1 GOOS=linux go build -trimpath -ldflags="-s -w" -o /out/cyclops-db-migrate ./cmd/db-migrate \
    && CGO_ENABLED=1 GOOS=linux go build -trimpath -ldflags="-s -w" -o /out/cyclops-usage-meter ./cmd/usage-meter

FROM --platform=linux/amd64 rust:1.94-bookworm AS rust-builder
WORKDIR /src/k8s-state-query
COPY src/services/k8s-state-query/Cargo.toml src/services/k8s-state-query/Cargo.lock ./
COPY src/services/k8s-state-query/src ./src
RUN cargo build --release --locked --bin k8s-state-projector && \
    cp target/release/k8s-state-projector /out-k8s-state-projector

FROM gcr.io/distroless/cc-debian12:nonroot AS runtime
WORKDIR /app
COPY --from=go-builder /out/cyclops-cs-backend /app/cyclops-cs-backend
COPY --from=go-builder /out/cyclops-db-migrate /app/cyclops-db-migrate
COPY --from=go-builder /out/cyclops-usage-meter /app/cyclops-usage-meter
COPY --from=rust-builder /out-k8s-state-projector /app/k8s-state-projector
USER nonroot:nonroot
EXPOSE 8080
ENTRYPOINT ["/app/cyclops-cs-backend"]
