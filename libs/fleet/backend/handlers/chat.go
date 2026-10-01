package handlers

import (
	"bytes"
	"encoding/json"
	"errors"
	"io"
	"log/slog"
	"net/http"
	"strings"
	"time"

	"cyclops-cs-backend/auth"
	"cyclops-cs-backend/chat"
)

const (
	chatTurnBodyMaxBytes               = 256 << 10
	chatConversationUpdateBodyMaxBytes = 4 << 10
	chatMessageMaxBytes                = 128 << 10
	chatHistoryMaxBytes                = 1 << 20
	chatHistoryMaxCount                = 256
)

var errChatTurnTooLarge = errors.New("chat turn is too large")

type TurnRequest struct {
	Messages []chat.Message `json:"messages"`
}

type ArchiveConversationRequest struct {
	Archived *bool `json:"archived" binding:"required"`
}

type turnEvent struct {
	Type    string        `json:"type"`
	Delta   string        `json:"delta,omitempty"`
	Message *chat.Message `json:"message,omitempty"`
}

func (h Handlers) chatUser(w http.ResponseWriter, r *http.Request) *auth.User {
	user := currentUser(r)
	if user == nil || user.ID == "" {
		writeErr(w, http.StatusUnauthorized, "missing user")
		return nil
	}
	enabled, err := h.chatEnabled(r.Context(), user)
	if err != nil {
		slog.WarnContext(r.Context(), "chat access eval failed; defaulting off", "err", err, "user", user.ID)
	}
	if err != nil || !enabled {
		writeErr(w, http.StatusNotFound, "chat is disabled")
		return nil
	}
	if h.Conversations == nil {
		writeErr(w, http.StatusServiceUnavailable, "chat is not configured")
		return nil
	}
	return user
}

// CreateConversation godoc
//
// @Summary      Create a chat conversation
// @Tags         chat
// @Produce      json
// @Success      201 {object} chat.Conversation
// @Failure      401 {object} ErrorResponse
// @Failure      404 {object} ErrorResponse
// @Failure      413 {object} ErrorResponse
// @Failure      503 {object} ErrorResponse
// @Security     BearerAuth
// @Router       /api/chat/conversations [post]
func (h Handlers) CreateConversation(w http.ResponseWriter, r *http.Request) {
	user := h.chatUser(w, r)
	if user == nil {
		return
	}
	conversation, err := h.Conversations.Create(r.Context(), user.ID)
	if errors.Is(err, chat.ErrConversationLimit) {
		writeErr(w, http.StatusTooManyRequests, "conversation limit reached")
		return
	}
	if err != nil {
		slog.ErrorContext(r.Context(), "create chat conversation", "err", err, "user", user.ID)
		writeErr(w, http.StatusInternalServerError, "failed to create conversation")
		return
	}
	writeJSON(w, http.StatusCreated, conversation)
}

// ListConversations godoc
//
// @Summary      List the calling user's chat conversations
// @Tags         chat
// @Produce      json
// @Param        archived query bool false "Whether to list archived conversations"
// @Success      200 {array} chat.ConversationSummary
// @Failure      400 {object} ErrorResponse
// @Failure      401 {object} ErrorResponse
// @Failure      404 {object} ErrorResponse
// @Failure      503 {object} ErrorResponse
// @Security     BearerAuth
// @Router       /api/chat/conversations [get]
func (h Handlers) ListConversations(w http.ResponseWriter, r *http.Request) {
	user := h.chatUser(w, r)
	if user == nil {
		return
	}
	archived := false
	if values, present := r.URL.Query()["archived"]; present {
		if len(values) != 1 {
			writeErr(w, http.StatusBadRequest, "invalid archived query value")
			return
		}
		switch values[0] {
		case "false":
		case "true":
			archived = true
		default:
			writeErr(w, http.StatusBadRequest, "invalid archived query value")
			return
		}
	}
	conversations, err := h.Conversations.List(r.Context(), user.ID, archived)
	if err != nil {
		slog.ErrorContext(r.Context(), "list chat conversations", "err", err, "user", user.ID)
		writeErr(w, http.StatusInternalServerError, "failed to list conversations")
		return
	}
	writeJSON(w, http.StatusOK, conversations)
}

// GetConversation godoc
//
// @Summary      Get a chat conversation
// @Tags         chat
// @Produce      json
// @Param        id path string true "Conversation ID"
// @Success      200 {object} chat.Conversation
// @Failure      401 {object} ErrorResponse
// @Failure      404 {object} ErrorResponse
// @Failure      503 {object} ErrorResponse
// @Security     BearerAuth
// @Router       /api/chat/conversations/{id} [get]
func (h Handlers) GetConversation(w http.ResponseWriter, r *http.Request) {
	user := h.chatUser(w, r)
	if user == nil {
		return
	}
	conversation, err := h.Conversations.Get(r.Context(), user.ID, chatConversationID(r))
	if errors.Is(err, chat.ErrConversationNotFound) {
		writeErr(w, http.StatusNotFound, "conversation not found")
		return
	}
	if err != nil {
		slog.ErrorContext(r.Context(), "get chat conversation", "err", err, "user", user.ID)
		writeErr(w, http.StatusInternalServerError, "failed to get conversation")
		return
	}
	writeJSON(w, http.StatusOK, conversation)
}

// UpdateConversation godoc
//
// @Summary      Archive or restore a chat conversation
// @Tags         chat
// @Accept       json
// @Produce      json
// @Param        id path string true "Conversation ID"
// @Param        body body ArchiveConversationRequest true "Archive state"
// @Success      200 {object} chat.Conversation
// @Failure      400 {object} ErrorResponse
// @Failure      401 {object} ErrorResponse
// @Failure      404 {object} ErrorResponse
// @Failure      413 {object} ErrorResponse
// @Failure      503 {object} ErrorResponse
// @Security     BearerAuth
// @Router       /api/chat/conversations/{id} [patch]
func (h Handlers) UpdateConversation(w http.ResponseWriter, r *http.Request) {
	user := h.chatUser(w, r)
	if user == nil {
		return
	}

	r.Body = http.MaxBytesReader(w, r.Body, chatConversationUpdateBodyMaxBytes)
	request, err := decodeArchiveConversationRequest(r.Body)
	if err != nil {
		var maxErr *http.MaxBytesError
		if errors.As(err, &maxErr) {
			writeErr(w, http.StatusRequestEntityTooLarge, "conversation update body is too large")
		} else {
			writeErr(w, http.StatusBadRequest, "invalid body")
		}
		return
	}

	conversationID := chatConversationID(r)
	unlock := h.lockConversation(conversationID)
	defer unlock()
	conversation, err := h.Conversations.SetArchived(r.Context(), user.ID, conversationID, *request.Archived)
	if errors.Is(err, chat.ErrConversationNotFound) {
		writeErr(w, http.StatusNotFound, "conversation not found")
		return
	}
	if err != nil {
		slog.ErrorContext(r.Context(), "update chat conversation archive state", "err", err, "user", user.ID)
		writeErr(w, http.StatusInternalServerError, "failed to update conversation")
		return
	}
	writeJSON(w, http.StatusOK, conversation)
}

// CreateTurn godoc
//
// @Summary      Send a chat turn and stream the assistant response
// @Tags         chat
// @Accept       json
// @Produce      application/x-ndjson
// @Param        id path string true "Conversation ID"
// @Param        body body TurnRequest true "User message or tool results"
// @Success      200 {object} turnEvent
// @Failure      400 {object} ErrorResponse
// @Failure      401 {object} ErrorResponse
// @Failure      404 {object} ErrorResponse
// @Failure      409 {object} ErrorResponse
// @Failure      503 {object} ErrorResponse
// @Security     BearerAuth
// @Router       /api/chat/conversations/{id}/turns [post]
func (h Handlers) CreateTurn(w http.ResponseWriter, r *http.Request) {
	user := h.chatUser(w, r)
	if user == nil {
		return
	}
	if h.Model == nil {
		writeErr(w, http.StatusServiceUnavailable, "chat is not configured")
		return
	}

	r.Body = http.MaxBytesReader(w, r.Body, chatTurnBodyMaxBytes)
	request, err := decodeTurnRequest(r.Body)
	if err != nil {
		var maxErr *http.MaxBytesError
		if errors.As(err, &maxErr) {
			writeErr(w, http.StatusRequestEntityTooLarge, "chat request body is too large")
		} else {
			writeErr(w, http.StatusBadRequest, "invalid body")
		}
		return
	}

	conversationID := chatConversationID(r)
	unlock := h.lockConversation(conversationID)
	defer unlock()
	conversation, err := h.Conversations.Get(r.Context(), user.ID, conversationID)
	if errors.Is(err, chat.ErrConversationNotFound) {
		writeErr(w, http.StatusNotFound, "conversation not found")
		return
	}
	if err != nil {
		slog.ErrorContext(r.Context(), "get chat conversation for turn", "err", err, "user", user.ID)
		writeErr(w, http.StatusInternalServerError, "failed to get conversation")
		return
	}
	if conversation.ArchivedAt != nil {
		writeErr(w, http.StatusConflict, "conversation is archived")
		return
	}
	request.Messages = recoverAbandonedToolCalls(conversation.Messages, request.Messages)
	if err := validateTurn(conversation.Messages, request.Messages); err != nil {
		if errors.Is(err, errChatTurnTooLarge) {
			writeErr(w, http.StatusRequestEntityTooLarge, err.Error())
		} else {
			writeErr(w, http.StatusBadRequest, err.Error())
		}
		return
	}
	if len(request.Messages) > 0 {
		if err := h.Conversations.Append(r.Context(), user.ID, conversationID, request.Messages...); err != nil {
			if errors.Is(err, chat.ErrConversationArchived) {
				writeErr(w, http.StatusConflict, "conversation is archived")
				return
			}
			if errors.Is(err, chat.ErrConversationLimit) {
				writeErr(w, http.StatusRequestEntityTooLarge, "conversation storage limit reached")
				return
			}
			if errors.Is(err, chat.ErrConversationNotFound) {
				writeErr(w, http.StatusNotFound, "conversation not found")
				return
			}
			slog.ErrorContext(r.Context(), "append chat input", "err", err, "user", user.ID)
			writeErr(w, http.StatusInternalServerError, "failed to append chat input")
			return
		}
		conversation, err = h.Conversations.Get(r.Context(), user.ID, conversationID)
		if err != nil {
			slog.ErrorContext(r.Context(), "reload chat conversation", "err", err, "user", user.ID)
			writeErr(w, http.StatusInternalServerError, "failed to load conversation")
			return
		}
	}

	w.Header().Set("Content-Type", "application/x-ndjson")
	w.WriteHeader(http.StatusOK)
	flusher, _ := w.(http.Flusher)
	emit := func(event turnEvent) error {
		if err := json.NewEncoder(w).Encode(event); err != nil {
			return err
		}
		if flusher != nil {
			flusher.Flush()
		}
		return nil
	}
	message, err := h.Model.Complete(r.Context(), conversation.Messages, func(delta string) error {
		return emit(turnEvent{Type: "content_delta", Delta: delta})
	})
	if err != nil {
		slog.WarnContext(r.Context(), "complete chat turn", "err", err, "user", user.ID)
		return
	}
	message.Role = chat.RoleAssistant
	message.ID = ""
	message.CreatedAt = time.Time{}
	if err := h.Conversations.Append(r.Context(), user.ID, conversationID, message); err != nil {
		if errors.Is(err, chat.ErrConversationLimit) {
			slog.WarnContext(r.Context(), "chat response exceeded storage limit", "user", user.ID)
			return
		}
		slog.ErrorContext(r.Context(), "append chat response", "err", err, "user", user.ID)
		return
	}
	_ = emit(turnEvent{Type: "assistant", Message: &message})
}

func recoverAbandonedToolCalls(history, messages []chat.Message) []chat.Message {
	outstanding := outstandingToolCalls(history)
	if len(outstanding) == 0 || (len(messages) > 0 && messages[0].Role == chat.RoleTool) {
		return messages
	}
	if len(messages) > 1 || (len(messages) == 1 && messages[0].Role != chat.RoleUser) {
		return messages
	}

	recovered := make([]chat.Message, 0, len(outstanding)+len(messages))
	for _, call := range history[len(history)-1].ToolCalls {
		result, _ := json.Marshal(bashToolResult{
			Stdout:    pointer(""),
			Stderr:    pointer("tool call abandoned before completion"),
			ExitCode:  pointer(1),
			TimedOut:  pointer(false),
			Truncated: pointer(false),
		})
		recovered = append(recovered, chat.Message{Role: chat.RoleTool, ToolCallID: call.ID, Content: string(result)})
	}
	return append(recovered, messages...)
}

func pointer[T any](value T) *T {
	return &value
}

func chatHistorySize(messages []chat.Message) int {
	total := 0
	for _, message := range messages {
		total += len(message.Content) + len(message.ToolCallID)
		for _, call := range message.ToolCalls {
			total += len(call.ID) + len(call.Type) + len(call.Function.Name) + len(call.Function.Arguments)
		}
	}
	return total
}

func validateTurn(history, messages []chat.Message) error {
	if len(messages) > 32 || len(history)+len(messages)+1 > chatHistoryMaxCount {
		return errChatTurnTooLarge
	}
	if chatHistorySize(history)+chatHistorySize(messages) > chatHistoryMaxBytes-chatMessageMaxBytes {
		return errChatTurnTooLarge
	}
	for _, message := range messages {
		if len(message.Content) > chatMessageMaxBytes {
			return errChatTurnTooLarge
		}
		if message.ID != "" || !message.CreatedAt.IsZero() || message.Role == chat.RoleAssistant || message.Role == "" {
			return errors.New("invalid client message")
		}
		if message.Role == chat.RoleUser {
			if message.ToolCallID != "" || len(message.ToolCalls) != 0 || strings.TrimSpace(message.Content) == "" {
				return errors.New("invalid user message")
			}
			continue
		}
		if message.Role != chat.RoleTool || message.ToolCallID == "" || len(message.ToolCalls) != 0 {
			return errors.New("invalid tool message")
		}
		if err := validateBashToolResult(message.Content); err != nil {
			return errors.Join(errors.New("invalid tool result"), err)

		}
	}

	if len(messages) == 0 {
		if len(history) == 0 || (history[len(history)-1].Role != chat.RoleUser && history[len(history)-1].Role != chat.RoleTool) {
			return errors.New("empty retry is not allowed")
		}
		return nil
	}

	outstanding := outstandingToolCalls(history)
	if len(outstanding) == 0 {
		if len(messages) != 1 || messages[0].Role != chat.RoleUser {
			return errors.New("expected exactly one user message")
		}
		return nil
	}
	toolMessages := messages
	if len(messages) == len(outstanding)+1 && messages[len(messages)-1].Role == chat.RoleUser {
		toolMessages = messages[:len(messages)-1]
	}
	if len(toolMessages) != len(outstanding) {
		return errors.New("tool results must match outstanding tool calls")
	}
	seen := make(map[string]struct{}, len(toolMessages))
	for _, message := range toolMessages {
		if message.Role != chat.RoleTool {
			return errors.New("tool results must match outstanding tool calls")
		}
		if _, ok := outstanding[message.ToolCallID]; !ok {
			return errors.New("tool result does not match an outstanding tool call")
		}
		if _, duplicate := seen[message.ToolCallID]; duplicate {
			return errors.New("duplicate tool result")
		}
		seen[message.ToolCallID] = struct{}{}
	}
	return nil
}

func outstandingToolCalls(history []chat.Message) map[string]struct{} {
	if len(history) == 0 || history[len(history)-1].Role != chat.RoleAssistant {
		return nil
	}
	calls := history[len(history)-1].ToolCalls
	if len(calls) == 0 {
		return nil
	}
	outstanding := make(map[string]struct{}, len(calls))
	for _, call := range calls {
		outstanding[call.ID] = struct{}{}
	}
	return outstanding
}

func chatConversationID(r *http.Request) string {
	if id := r.PathValue("id"); id != "" {
		return id
	}
	const prefix = "/api/chat/conversations/"
	path := strings.TrimPrefix(r.URL.Path, prefix)
	return strings.TrimSuffix(path, "/turns")
}

func decodeArchiveConversationRequest(body io.Reader) (ArchiveConversationRequest, error) {
	var request ArchiveConversationRequest
	decoder := json.NewDecoder(body)
	decoder.DisallowUnknownFields()
	if err := decoder.Decode(&request); err != nil {
		return ArchiveConversationRequest{}, err
	}
	if err := decoder.Decode(&struct{}{}); err != io.EOF {
		return ArchiveConversationRequest{}, errors.New("multiple JSON values")
	}
	if request.Archived == nil {
		return ArchiveConversationRequest{}, errors.New("archived is required")
	}
	return request, nil
}

func decodeTurnRequest(body io.Reader) (TurnRequest, error) {
	var raw struct {
		Messages *json.RawMessage `json:"messages"`
	}
	decoder := json.NewDecoder(body)
	decoder.DisallowUnknownFields()
	if err := decoder.Decode(&raw); err != nil {
		return TurnRequest{}, err
	}
	if err := decoder.Decode(&struct{}{}); err != io.EOF {
		return TurnRequest{}, errors.New("multiple JSON values")
	}

	if raw.Messages == nil {
		return TurnRequest{}, errors.New("messages is required")
	}
	var rawMessages []json.RawMessage
	if err := json.Unmarshal(*raw.Messages, &rawMessages); err != nil || rawMessages == nil {
		return TurnRequest{}, errors.Join(errors.New("messages must be an array"), err)

	}

	request := TurnRequest{Messages: make([]chat.Message, 0, len(rawMessages))}
	for _, rawMessage := range rawMessages {
		var fields map[string]json.RawMessage
		if err := json.Unmarshal(rawMessage, &fields); err != nil {
			return TurnRequest{}, err
		}
		if _, ok := fields["id"]; ok {
			return TurnRequest{}, errors.New("client message id is not allowed")
		}
		if _, ok := fields["created_at"]; ok {
			return TurnRequest{}, errors.New("client message timestamp is not allowed")
		}

		var message chat.Message
		messageDecoder := json.NewDecoder(bytes.NewReader(rawMessage))
		messageDecoder.DisallowUnknownFields()
		if err := messageDecoder.Decode(&message); err != nil {
			return TurnRequest{}, err
		}
		request.Messages = append(request.Messages, message)
	}
	return request, nil
}

type bashToolResult struct {
	Stdout    *string `json:"stdout"`
	Stderr    *string `json:"stderr"`
	ExitCode  *int    `json:"exit_code"`
	TimedOut  *bool   `json:"timed_out"`
	Truncated *bool   `json:"truncated"`
}

func validateBashToolResult(content string) error {
	var result bashToolResult
	decoder := json.NewDecoder(strings.NewReader(content))
	decoder.DisallowUnknownFields()
	if err := decoder.Decode(&result); err != nil {
		return err
	}
	if err := decoder.Decode(&struct{}{}); err != io.EOF {
		return errors.New("trailing JSON")
	}
	if result.Stdout == nil || result.Stderr == nil || result.ExitCode == nil || result.TimedOut == nil || result.Truncated == nil {
		return errors.New("missing BashToolResult field")
	}
	return nil
}
