package handlers

import (
	"context"
	"encoding/json"
	"errors"
	"io"
	"log/slog"
	"mime"
	"net/http"

	"cyclops-cs-backend/auth"
	"cyclops-cs-backend/identity"
	"cyclops-cs-backend/statequery"
	"github.com/jackc/pgx/v5/pgconn"
)

const stateQueryBodyMaxBytes = 1 << 20

type StateQueryExecutor interface {
	Execute(context.Context, string, string, statequery.ResultWriter) error
}

type stateQueryStreamWriter struct {
	response   http.ResponseWriter
	encoder    *json.Encoder
	controller *http.ResponseController
	started    bool
}

type stateQueryFieldsEvent struct {
	Type   string                    `json:"type"`
	Fields []pgconn.FieldDescription `json:"fields"`
}

type stateQueryRowEvent struct {
	Type   string `json:"type"`
	Values []any  `json:"values"`
}

type stateQueryDoneEvent struct {
	Type string `json:"type"`
}

type stateQueryErrorEvent struct {
	Type  string `json:"type"`
	Error string `json:"error"`
}

func newStateQueryStreamWriter(response http.ResponseWriter) *stateQueryStreamWriter {
	return &stateQueryStreamWriter{
		response:   response,
		encoder:    json.NewEncoder(response),
		controller: http.NewResponseController(response),
	}
}

func (writer *stateQueryStreamWriter) WriteFieldDescriptions(fields []pgconn.FieldDescription) error {
	return writer.write(stateQueryFieldsEvent{Type: "fields", Fields: fields})
}

func (writer *stateQueryStreamWriter) WriteRow(values []any) error {
	return writer.write(stateQueryRowEvent{Type: "row", Values: values})
}

func (writer *stateQueryStreamWriter) Finish(err error) error {
	if err != nil {
		if auth.IsDatabaseUnavailable(err) {
			return errors.Join(writer.write(stateQueryErrorEvent{Type: "error", Error: "state query unavailable"}), err)

		}
		return errors.Join(writer.write(stateQueryErrorEvent{Type: "error", Error: err.Error()}), err)

	}
	return writer.write(stateQueryDoneEvent{Type: "done"})
}

func (writer *stateQueryStreamWriter) write(event any) error {
	if !writer.started {
		writer.response.Header().Set("Content-Type", "application/x-ndjson")
		writer.response.WriteHeader(http.StatusOK)
		writer.started = true
	}
	if err := writer.encoder.Encode(event); err != nil {
		return err
	}
	return writer.controller.Flush()
}

func (h Handlers) QueryState(w http.ResponseWriter, r *http.Request) {
	w.Header().Set("Accept-Query", "application/sql")
	w.Header().Set("Cache-Control", "private, no-store")
	executor := h.Features.StateQuery()
	if executor == nil {
		writeErr(w, http.StatusServiceUnavailable, "state query unavailable")
		return
	}
	mediaType, _, err := mime.ParseMediaType(r.Header.Get("Content-Type"))
	if err != nil {
		w.Header().Set("Accept", "application/sql")
		writeErr(w, http.StatusUnsupportedMediaType, err.Error())
		return
	}
	if mediaType != "application/sql" {
		w.Header().Set("Accept", "application/sql")
		writeErr(w, http.StatusUnsupportedMediaType, "state query requires application/sql")
		return
	}
	body, err := io.ReadAll(http.MaxBytesReader(w, r.Body, stateQueryBodyMaxBytes))
	if err != nil {
		writeErr(w, http.StatusBadRequest, err.Error())
		return
	}

	user := currentUser(r)
	stream := newStateQueryStreamWriter(w)
	ctx, cancel := databaseContext(r.Context())
	defer cancel()
	err = executor.Execute(ctx, identity.PersonalGroup(r.Context(), user.ID), string(body), stream)
	if err != nil && !stream.started {
		writeErr(w, http.StatusServiceUnavailable, "state query unavailable")
		return
	}
	err = stream.Finish(err)
	if err != nil {
		slog.Debug("state query stream failed", "err", err)
	}
}
