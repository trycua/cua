package handlers

import (
	"context"
	"encoding/json"
	"fmt"
	"net/http"
	"net/http/httptest"
	"strings"
	"sync"
	"testing"
	"time"

	"cyclops-cs-backend/auth"
	"cyclops-cs-backend/chat"
)

type fakeModel struct {
	responses []chat.Message
	histories [][]chat.Message
}

var alice = &auth.User{ID: "user-alice", AZP: "cyclops-cs-spa"}

func newReq(method, target, body string, user *auth.User) *http.Request {
	request := httptest.NewRequest(method, target, strings.NewReader(body))
	parts := strings.Split(strings.Trim(target, "/"), "/")
	if len(parts) >= 4 && parts[0] == "api" && parts[1] == "chat" && parts[2] == "conversations" {
		request.SetPathValue("id", parts[3])
	}
	if user != nil {
		request = withUser(request, user)
	}
	return request
}

func (f *fakeModel) Complete(_ context.Context, messages []chat.Message, onDelta func(string) error) (chat.Message, error) {
	f.histories = append(f.histories, append([]chat.Message(nil), messages...))
	response := f.responses[0]
	f.responses = f.responses[1:]
	if response.Content != "" {
		_ = onDelta(response.Content)
	}
	return response, nil
}

func newChatHandlers(responses ...chat.Message) (Handlers, *chat.MemoryConversationStore, *fakeModel) {
	store := chat.NewMemoryConversationStore()
	model := &fakeModel{responses: responses}
	return Handlers{
		Conversations: store,
		Model:         model,
		chatAccessEvaluator: func(context.Context, *auth.User) (bool, error) {
			return true, nil
		},
		chatLocks: newConversationLockRegistry(),
	}, store, model
}

func createChatConversation(t *testing.T, h Handlers, user *auth.User) *chat.Conversation {
	t.Helper()
	w := httptest.NewRecorder()
	h.CreateConversation(w, newReq(http.MethodPost, "/api/chat/conversations", "", user))
	if w.Code != http.StatusCreated {
		t.Fatalf("create status = %d, want 201; body = %s", w.Code, w.Body.String())
	}
	var conversation chat.Conversation
	if err := json.Unmarshal(w.Body.Bytes(), &conversation); err != nil {
		t.Fatalf("decode conversation: %v", err)
	}
	return &conversation
}

func TestChatConversationsCreateListGetOwnership(t *testing.T) {
	h, _, _ := newChatHandlers()
	conversation := createChatConversation(t, h, alice)
	w := httptest.NewRecorder()
	h.ListConversations(w, newReq(http.MethodGet, "/api/chat/conversations", "", alice))
	if w.Code != http.StatusOK {
		t.Fatalf("list status = %d, want 200; body = %s", w.Code, w.Body.String())
	}
	var summaries []chat.ConversationSummary
	if err := json.Unmarshal(w.Body.Bytes(), &summaries); err != nil {
		t.Fatalf("decode summaries: %v", err)
	}
	if len(summaries) != 1 || summaries[0].ID != conversation.ID {
		t.Fatalf("summaries = %#v", summaries)
	}
	w = httptest.NewRecorder()
	h.GetConversation(w, newReq(http.MethodGet, "/api/chat/conversations/"+conversation.ID, "", alice))
	if w.Code != http.StatusOK {
		t.Fatalf("get status = %d, want 200; body = %s", w.Code, w.Body.String())
	}
	w = httptest.NewRecorder()
	h.GetConversation(w, newReq(http.MethodGet, "/api/chat/conversations/"+conversation.ID, "", &auth.User{ID: "user-bob"}))
	if w.Code != http.StatusNotFound {
		t.Fatalf("cross-owner get status = %d, want 404; body = %s", w.Code, w.Body.String())
	}
	w = httptest.NewRecorder()
	h.GetConversation(w, newReq(http.MethodGet, "/api/chat/conversations/missing", "", alice))
	if w.Code != http.StatusNotFound {
		t.Fatalf("missing get status = %d, want 404; body = %s", w.Code, w.Body.String())
	}
}

func TestChatRequiresEnabledFeatureAndUser(t *testing.T) {
	w := httptest.NewRecorder()
	disabled := Handlers{chatAccessEvaluator: func(context.Context, *auth.User) (bool, error) { return false, nil }}
	disabled.ListConversations(w, newReq(http.MethodGet, "/api/chat/conversations", "", alice))
	if w.Code != http.StatusNotFound {
		t.Fatalf("disabled status = %d, want 404", w.Code)
	}
	h, _, _ := newChatHandlers()
	for _, user := range []*auth.User{nil, &auth.User{}} {
		w = httptest.NewRecorder()
		h.ListConversations(w, newReq(http.MethodGet, "/api/chat/conversations", "", user))
		if w.Code != http.StatusUnauthorized {
			t.Fatalf("user %#v status = %d, want 401", user, w.Code)
		}
	}
}

func TestChatTurnRejectsInvalidClientMessages(t *testing.T) {
	cases := map[string]string{
		"assistant":              `{"messages":[{"role":"assistant","content":"no"}]}`,
		"client id":              `{"messages":[{"id":"client-id","role":"user","content":"hello"}]}`,
		"client timestamp":       `{"messages":[{"role":"user","content":"hello","created_at":"2026-01-01T00:00:00Z"}]}`,
		"user tool call":         `{"messages":[{"role":"user","content":"hello","tool_calls":[{"id":"call-1"}]}]}`,
		"empty client id":        `{"messages":[{"id":"","role":"user","content":"hello"}]}`,
		"tool missing call id":   `{"messages":[{"role":"tool","content":"output"}]}`,
		"zero client timestamp":  `{"messages":[{"role":"user","content":"hello","created_at":"0001-01-01T00:00:00Z"}]}`,
		"tool has tool calls":    `{"messages":[{"role":"tool","tool_call_id":"call-1","tool_calls":[{"id":"call-1"}]}]}`,
		"multiple user messages": `{"messages":[{"role":"user","content":"one"},{"role":"user","content":"two"}]}`,
	}
	for name, body := range cases {
		t.Run(name, func(t *testing.T) {
			h, _, _ := newChatHandlers(chat.Message{Role: chat.RoleAssistant, Content: "unused"})
			conversation := createChatConversation(t, h, alice)
			w := httptest.NewRecorder()
			h.CreateTurn(w, newReq(http.MethodPost, "/api/chat/conversations/"+conversation.ID+"/turns", body, alice))
			if w.Code != http.StatusBadRequest {
				t.Fatalf("status = %d, want 400; body = %s", w.Code, w.Body.String())
			}
		})
	}
}

func TestChatTurnStreamsAndPersistsMessages(t *testing.T) {
	h, store, _ := newChatHandlers(chat.Message{Role: chat.RoleAssistant, Content: "hello"})
	conversation := createChatConversation(t, h, alice)
	w := httptest.NewRecorder()
	h.CreateTurn(w, newReq(http.MethodPost, "/api/chat/conversations/"+conversation.ID+"/turns", `{"messages":[{"role":"user","content":"hi"}]}`, alice))
	if w.Code != http.StatusOK {
		t.Fatalf("status = %d, want 200; body = %s", w.Code, w.Body.String())
	}
	if got, want := w.Header().Get("Content-Type"), "application/x-ndjson"; got != want {
		t.Fatalf("Content-Type = %q, want %q", got, want)
	}
	if got, want := w.Body.String(), "{\"type\":\"content_delta\",\"delta\":\"hello\"}\n{\"type\":\"assistant\",\"message\":{\"role\":\"assistant\",\"content\":\"hello\"}}\n"; got != want {
		t.Fatalf("stream = %q, want %q", got, want)
	}
	stored, err := store.Get(context.Background(), alice.ID, conversation.ID)
	if err != nil {
		t.Fatal(err)
	}
	if len(stored.Messages) != 2 || stored.Messages[0].Role != chat.RoleUser || stored.Messages[0].Content != "hi" || stored.Messages[1].Role != chat.RoleAssistant || stored.Messages[1].Content != "hello" {
		t.Fatalf("stored messages = %#v", stored.Messages)
	}
}

func TestChatTurnAbandonsOutstandingToolCallsBeforeUserMessage(t *testing.T) {
	h, store, model := newChatHandlers(chat.Message{Role: chat.RoleAssistant, Content: "continued"})
	conversation := createChatConversation(t, h, alice)
	if err := store.Append(context.Background(), alice.ID, conversation.ID, chat.Message{Role: chat.RoleAssistant, ToolCalls: []chat.ToolCall{{ID: "call-1", Type: "function", Function: chat.ToolFunction{Name: "run_bash", Arguments: `{"command":"pwd"}`}}}}); err != nil {
		t.Fatal(err)
	}
	w := httptest.NewRecorder()
	h.CreateTurn(w, newReq(http.MethodPost, "/api/chat/conversations/"+conversation.ID+"/turns", `{"messages":[{"role":"user","content":"skip tools"}]}`, alice))
	if w.Code != http.StatusOK {
		t.Fatalf("status = %d, want 200; body = %s", w.Code, w.Body.String())
	}
	if len(model.histories) != 1 || len(model.histories[0]) != 3 || model.histories[0][1].Role != chat.RoleTool || model.histories[0][2].Content != "skip tools" {
		t.Fatalf("model history = %#v", model.histories)
	}
}

func TestChatTurnRequiresExactToolResultsAndSendsCompleteHistory(t *testing.T) {
	h, store, model := newChatHandlers(chat.Message{Role: chat.RoleAssistant, Content: "complete"})
	conversation := createChatConversation(t, h, alice)
	toolCalls := []chat.ToolCall{{ID: "call-1", Type: "function", Function: chat.ToolFunction{Name: "run_bash", Arguments: `{"command":"pwd"}`}}, {ID: "call-2", Type: "function", Function: chat.ToolFunction{Name: "run_bash", Arguments: `{"command":"ls"}`}}}
	if err := store.Append(context.Background(), alice.ID, conversation.ID, chat.Message{Role: chat.RoleAssistant, ToolCalls: toolCalls}); err != nil {
		t.Fatal(err)
	}
	for name, body := range map[string]string{
		"missing":    `{"messages":[{"role":"tool","tool_call_id":"call-1","content":"/tmp"}]}`,
		"duplicate":  `{"messages":[{"role":"tool","tool_call_id":"call-1","content":"/tmp"},{"role":"tool","tool_call_id":"call-1","content":"/tmp"}]}`,
		"extra":      `{"messages":[{"role":"tool","tool_call_id":"call-1","content":"/tmp"},{"role":"tool","tool_call_id":"extra","content":"no"}]}`,
		"mixed user": `{"messages":[{"role":"tool","tool_call_id":"call-1","content":"/tmp"},{"role":"user","content":"no"}]}`,
	} {
		t.Run(name, func(t *testing.T) {
			w := httptest.NewRecorder()
			h.CreateTurn(w, newReq(http.MethodPost, "/api/chat/conversations/"+conversation.ID+"/turns", body, alice))
			if w.Code != http.StatusBadRequest {
				t.Fatalf("status = %d, want 400; body = %s", w.Code, w.Body.String())
			}
		})
	}
	w := httptest.NewRecorder()
	body := fmt.Sprintf(`{"messages":[{"role":"tool","tool_call_id":"call-1","content":%q},{"role":"tool","tool_call_id":"call-2","content":%q}]}`, validBashToolResult, validBashToolResult)
	h.CreateTurn(w, newReq(http.MethodPost, "/api/chat/conversations/"+conversation.ID+"/turns", body, alice))
	if w.Code != http.StatusOK {
		t.Fatalf("status = %d, want 200; body = %s", w.Code, w.Body.String())
	}
	if len(model.histories) != 1 || len(model.histories[0]) != 3 {
		t.Fatalf("model histories = %#v", model.histories)
	}
	if got := model.histories[0][1:]; got[0].ToolCallID != "call-1" || got[1].ToolCallID != "call-2" {
		t.Fatalf("model tool history = %#v", got)
	}
}

func TestChatTurnAllowsEmptyRetryOnlyAfterUserOrTool(t *testing.T) {
	h, store, model := newChatHandlers(chat.Message{Role: chat.RoleAssistant, Content: "retry user"}, chat.Message{Role: chat.RoleAssistant, Content: "retry tool"})
	conversation := createChatConversation(t, h, alice)
	if err := store.Append(context.Background(), alice.ID, conversation.ID, chat.Message{Role: chat.RoleUser, Content: "retry"}); err != nil {
		t.Fatal(err)
	}
	w := httptest.NewRecorder()
	h.CreateTurn(w, newReq(http.MethodPost, "/api/chat/conversations/"+conversation.ID+"/turns", `{"messages":[]}`, alice))
	if w.Code != http.StatusOK {
		t.Fatalf("user retry status = %d, want 200; body = %s", w.Code, w.Body.String())
	}
	conversation = createChatConversation(t, h, alice)
	if err := store.Append(context.Background(), alice.ID, conversation.ID, chat.Message{Role: chat.RoleTool, ToolCallID: "call-1", Content: "retry"}); err != nil {
		t.Fatal(err)
	}
	w = httptest.NewRecorder()
	h.CreateTurn(w, newReq(http.MethodPost, "/api/chat/conversations/"+conversation.ID+"/turns", `{"messages":[]}`, alice))
	if w.Code != http.StatusOK {
		t.Fatalf("tool retry status = %d, want 200; body = %s", w.Code, w.Body.String())
	}
	conversation = createChatConversation(t, h, alice)
	if err := store.Append(context.Background(), alice.ID, conversation.ID, chat.Message{Role: chat.RoleAssistant, Content: "already answered"}); err != nil {
		t.Fatal(err)
	}
	w = httptest.NewRecorder()
	h.CreateTurn(w, newReq(http.MethodPost, "/api/chat/conversations/"+conversation.ID+"/turns", `{"messages":[]}`, alice))
	if w.Code != http.StatusBadRequest {
		t.Fatalf("assistant retry status = %d, want 400; body = %s", w.Code, w.Body.String())
	}
	if len(model.histories) != 2 {
		t.Fatalf("model calls = %d, want 2", len(model.histories))
	}
}

func TestChatTurnRejectsMalformedJSON(t *testing.T) {
	h, _, _ := newChatHandlers(chat.Message{Role: chat.RoleAssistant, Content: "unused"})
	conversation := createChatConversation(t, h, alice)
	w := httptest.NewRecorder()
	h.CreateTurn(w, newReq(http.MethodPost, "/api/chat/conversations/"+conversation.ID+"/turns", strings.Repeat("{", 1), alice))
	if w.Code != http.StatusBadRequest {
		t.Fatalf("status = %d, want 400; body = %s", w.Code, w.Body.String())
	}
}

func TestChatTurnRequiresMessagesArray(t *testing.T) {
	for name, body := range map[string]string{
		"missing": `{}`,
		"null":    `{"messages":null}`,
	} {
		t.Run(name, func(t *testing.T) {
			h, store, _ := newChatHandlers(chat.Message{Role: chat.RoleAssistant, Content: "unused"})
			conversation := createChatConversation(t, h, alice)
			if err := store.Append(context.Background(), alice.ID, conversation.ID, chat.Message{Role: chat.RoleUser, Content: "retry"}); err != nil {
				t.Fatal(err)
			}
			w := httptest.NewRecorder()
			h.CreateTurn(w, newReq(http.MethodPost, "/api/chat/conversations/"+conversation.ID+"/turns", body, alice))
			if w.Code != http.StatusBadRequest {
				t.Fatalf("status = %d, want 400; body = %s", w.Code, w.Body.String())
			}
		})
	}
}

const validBashToolResult = `{"stdout":"/tmp\n","stderr":"","exit_code":0,"timed_out":false,"truncated":false}`

func toolResultTurnBody(toolCallID, content string) string {
	return fmt.Sprintf(`{"messages":[{"role":"tool","tool_call_id":%q,"content":%q}]}`, toolCallID, content)
}

func TestChatTurnRejectsMalformedBashToolResults(t *testing.T) {
	cases := map[string]string{
		"non-JSON":      "output",
		"missing field": `{"stdout":"","stderr":"","exit_code":0,"timed_out":false}`,
		"wrong string":  `{"stdout":1,"stderr":"","exit_code":0,"timed_out":false,"truncated":false}`,
		"wrong integer": `{"stdout":"","stderr":"","exit_code":"0","timed_out":false,"truncated":false}`,
		"wrong boolean": `{"stdout":"","stderr":"","exit_code":0,"timed_out":"false","truncated":false}`,
		"unknown field": `{"stdout":"","stderr":"","exit_code":0,"timed_out":false,"truncated":false,"extra":true}`,
		"trailing JSON": validBashToolResult + ` {}`,
	}
	for name, content := range cases {
		t.Run(name, func(t *testing.T) {
			h, store, model := newChatHandlers(chat.Message{Role: chat.RoleAssistant, Content: "unused"})
			conversation := createChatConversation(t, h, alice)
			if err := store.Append(context.Background(), alice.ID, conversation.ID, chat.Message{Role: chat.RoleAssistant, ToolCalls: []chat.ToolCall{{ID: "call-1"}}}); err != nil {
				t.Fatal(err)
			}
			w := httptest.NewRecorder()
			h.CreateTurn(w, newReq(http.MethodPost, "/api/chat/conversations/"+conversation.ID+"/turns", toolResultTurnBody("call-1", content), alice))
			if w.Code != http.StatusBadRequest {
				t.Fatalf("status = %d, want 400; body = %s", w.Code, w.Body.String())
			}
			stored, err := store.Get(context.Background(), alice.ID, conversation.ID)
			if err != nil {
				t.Fatal(err)
			}
			if len(stored.Messages) != 1 || len(model.histories) != 0 {
				t.Fatalf("malformed tool result persisted or reached model: messages=%#v histories=%#v", stored.Messages, model.histories)
			}
		})
	}
}

func TestChatTurnAcceptsValidBashToolResult(t *testing.T) {
	h, store, model := newChatHandlers(chat.Message{Role: chat.RoleAssistant, Content: "complete"})
	conversation := createChatConversation(t, h, alice)
	if err := store.Append(context.Background(), alice.ID, conversation.ID, chat.Message{Role: chat.RoleAssistant, ToolCalls: []chat.ToolCall{{ID: "call-1"}}}); err != nil {
		t.Fatal(err)
	}
	w := httptest.NewRecorder()
	h.CreateTurn(w, newReq(http.MethodPost, "/api/chat/conversations/"+conversation.ID+"/turns", toolResultTurnBody("call-1", validBashToolResult), alice))
	if w.Code != http.StatusOK {
		t.Fatalf("status = %d, want 200; body = %s", w.Code, w.Body.String())
	}
	if len(model.histories) != 1 || len(model.histories[0]) != 2 {
		t.Fatalf("model histories = %#v", model.histories)
	}
}

type blockingSequentialModel struct {
	mu            sync.Mutex
	histories     [][]chat.Message
	calls         int
	firstStarted  chan struct{}
	secondStarted chan struct{}
	releaseFirst  chan struct{}
}

func (m *blockingSequentialModel) Complete(_ context.Context, messages []chat.Message, _ func(string) error) (chat.Message, error) {
	m.mu.Lock()
	m.calls++
	call := m.calls
	m.histories = append(m.histories, append([]chat.Message(nil), messages...))
	m.mu.Unlock()

	if call == 1 {
		close(m.firstStarted)
		<-m.releaseFirst
	} else if call == 2 {
		close(m.secondStarted)
	}
	return chat.Message{Role: chat.RoleAssistant, Content: fmt.Sprintf("assistant-%d", call)}, nil
}

func TestChatTurnSerializesConcurrentTurnsPerConversation(t *testing.T) {
	store := chat.NewMemoryConversationStore()
	model := &blockingSequentialModel{
		firstStarted:  make(chan struct{}),
		secondStarted: make(chan struct{}),
		releaseFirst:  make(chan struct{}),
	}
	h := Handlers{Conversations: store, Model: model, chatAccessEvaluator: func(context.Context, *auth.User) (bool, error) { return true, nil }}
	conversation := createChatConversation(t, h, alice)

	statuses := make(chan int, 2)
	go func() {
		w := httptest.NewRecorder()
		h.CreateTurn(w, newReq(http.MethodPost, "/api/chat/conversations/"+conversation.ID+"/turns", `{"messages":[{"role":"user","content":"first"}]}`, alice))
		statuses <- w.Code
	}()
	<-model.firstStarted
	go func() {
		w := httptest.NewRecorder()
		h.CreateTurn(w, newReq(http.MethodPost, "/api/chat/conversations/"+conversation.ID+"/turns", `{"messages":[{"role":"user","content":"second"}]}`, alice))
		statuses <- w.Code
	}()

	select {
	case <-model.secondStarted:
		close(model.releaseFirst)
		<-statuses
		<-statuses
		t.Fatal("second turn reached the model before the first turn completed")
	case <-time.After(100 * time.Millisecond):
	}
	close(model.releaseFirst)
	if first, second := <-statuses, <-statuses; first != http.StatusOK || second != http.StatusOK {
		t.Fatalf("statuses = %d, %d; want 200, 200", first, second)
	}

	model.mu.Lock()
	histories := append([][]chat.Message(nil), model.histories...)
	model.mu.Unlock()
	if len(histories) != 2 || len(histories[1]) != 3 {
		t.Fatalf("histories = %#v", histories)
	}
	if histories[1][0].Content != "first" || histories[1][1].Role != chat.RoleAssistant || histories[1][1].Content != "assistant-1" || histories[1][2].Content != "second" {
		t.Fatalf("second model history = %#v", histories[1])
	}
}

func TestChatConversationArchiveWaitsForActiveTurn(t *testing.T) {
	store := chat.NewMemoryConversationStore()
	model := &blockingSequentialModel{
		firstStarted: make(chan struct{}),
		releaseFirst: make(chan struct{}),
	}
	h := Handlers{
		Conversations: store,
		Model:         model,
		chatAccessEvaluator: func(context.Context, *auth.User) (bool, error) {
			return true, nil
		},
		chatLocks: newConversationLockRegistry(),
	}
	conversation := createChatConversation(t, h, alice)

	turnStatus := make(chan int, 1)
	go func() {
		w := httptest.NewRecorder()
		h.CreateTurn(w, newReq(http.MethodPost, "/api/chat/conversations/"+conversation.ID+"/turns", `{"messages":[{"role":"user","content":"hold the lock"}]}`, alice))
		turnStatus <- w.Code
	}()
	<-model.firstStarted

	archiveStatus := make(chan int, 1)
	go func() {
		w := httptest.NewRecorder()
		h.UpdateConversation(w, newReq(http.MethodPatch, "/api/chat/conversations/"+conversation.ID, `{"archived":true}`, alice))
		archiveStatus <- w.Code
	}()

	select {
	case status := <-archiveStatus:
		close(model.releaseFirst)
		<-turnStatus
		t.Fatalf("PATCH completed with status %d while the turn held the conversation lock", status)
	case <-time.After(100 * time.Millisecond):
	}

	close(model.releaseFirst)
	if status := <-turnStatus; status != http.StatusOK {
		t.Fatalf("turn status = %d, want 200", status)
	}
	if status := <-archiveStatus; status != http.StatusOK {
		t.Fatalf("PATCH status = %d, want 200", status)
	}
	updated, err := store.Get(context.Background(), alice.ID, conversation.ID)
	if err != nil {
		t.Fatalf("get archived conversation: %v", err)
	}
	if updated.ArchivedAt == nil {
		t.Fatal("archived_at = nil after PATCH")
	}
}

func TestChatTurnRecoversAbandonedToolCallsBeforeNewPrompt(t *testing.T) {
	h, store, model := newChatHandlers(chat.Message{Role: chat.RoleAssistant, Content: "recovered"})
	conversation := createChatConversation(t, h, alice)
	if err := store.Append(context.Background(), alice.ID, conversation.ID, chat.Message{
		Role:      chat.RoleAssistant,
		ToolCalls: []chat.ToolCall{{ID: "call-1", Type: "function", Function: chat.ToolFunction{Name: "bash", Arguments: `{"command":"sleep 60"}`}}},
	}); err != nil {
		t.Fatal(err)
	}

	w := httptest.NewRecorder()
	h.CreateTurn(w, newReq(http.MethodPost, "/api/chat/conversations/"+conversation.ID+"/turns", `{"messages":[{"role":"user","content":"continue without it"}]}`, alice))
	if w.Code != http.StatusOK {
		t.Fatalf("status = %d, want 200; body = %s", w.Code, w.Body.String())
	}
	if len(model.histories) != 1 || len(model.histories[0]) != 3 {
		t.Fatalf("model history = %#v", model.histories)
	}
	if model.histories[0][1].Role != chat.RoleTool || model.histories[0][1].ToolCallID != "call-1" || !strings.Contains(model.histories[0][1].Content, "abandoned") {
		t.Fatalf("synthetic tool result = %#v", model.histories[0][1])
	}
	if model.histories[0][2].Role != chat.RoleUser || model.histories[0][2].Content != "continue without it" {
		t.Fatalf("new prompt = %#v", model.histories[0][2])
	}
}

func TestChatTurnRetriesAbandonedToolCalls(t *testing.T) {
	h, store, model := newChatHandlers(chat.Message{Role: chat.RoleAssistant, Content: "recovered"})
	conversation := createChatConversation(t, h, alice)
	if err := store.Append(context.Background(), alice.ID, conversation.ID, chat.Message{
		Role:      chat.RoleAssistant,
		ToolCalls: []chat.ToolCall{{ID: "call-1", Type: "function", Function: chat.ToolFunction{Name: "bash", Arguments: `{"command":"bad"}`}}},
	}); err != nil {
		t.Fatal(err)
	}

	w := httptest.NewRecorder()
	h.CreateTurn(w, newReq(http.MethodPost, "/api/chat/conversations/"+conversation.ID+"/turns", `{"messages":[]}`, alice))
	if w.Code != http.StatusOK {
		t.Fatalf("status = %d, want 200; body = %s", w.Code, w.Body.String())
	}
	if len(model.histories) != 1 || len(model.histories[0]) != 2 || model.histories[0][1].Role != chat.RoleTool {
		t.Fatalf("model history = %#v", model.histories)
	}
}

func TestChatTurnRejectsOversizedBodyAndMessages(t *testing.T) {
	t.Run("body", func(t *testing.T) {
		h, _, _ := newChatHandlers(chat.Message{Role: chat.RoleAssistant, Content: "unused"})
		conversation := createChatConversation(t, h, alice)
		body := `{"messages":[{"role":"user","content":"` + strings.Repeat("x", 300<<10) + `"}]}`
		w := httptest.NewRecorder()
		h.CreateTurn(w, newReq(http.MethodPost, "/api/chat/conversations/"+conversation.ID+"/turns", body, alice))
		if w.Code != http.StatusRequestEntityTooLarge {
			t.Fatalf("status = %d, want 413; body = %s", w.Code, w.Body.String())
		}
	})

	t.Run("message", func(t *testing.T) {
		h, _, _ := newChatHandlers(chat.Message{Role: chat.RoleAssistant, Content: "unused"})
		conversation := createChatConversation(t, h, alice)
		body := `{"messages":[{"role":"user","content":"` + strings.Repeat("x", 140<<10) + `"}]}`
		w := httptest.NewRecorder()
		h.CreateTurn(w, newReq(http.MethodPost, "/api/chat/conversations/"+conversation.ID+"/turns", body, alice))
		if w.Code != http.StatusRequestEntityTooLarge {
			t.Fatalf("status = %d, want 413; body = %s", w.Code, w.Body.String())
		}
	})
}

func TestChatTurnRejectsConversationBeyondHistoryLimit(t *testing.T) {
	h, store, _ := newChatHandlers(chat.Message{Role: chat.RoleAssistant, Content: "unused"})
	conversation := createChatConversation(t, h, alice)
	messages := make([]chat.Message, 256)
	for index := range messages {
		messages[index] = chat.Message{Role: chat.RoleUser, Content: "message"}
	}
	if err := store.Append(context.Background(), alice.ID, conversation.ID, messages...); err != nil {
		t.Fatal(err)
	}
	w := httptest.NewRecorder()
	h.CreateTurn(w, newReq(http.MethodPost, "/api/chat/conversations/"+conversation.ID+"/turns", `{"messages":[{"role":"user","content":"one more"}]}`, alice))
	if w.Code != http.StatusRequestEntityTooLarge {
		t.Fatalf("status = %d, want 413; body = %s", w.Code, w.Body.String())
	}
}

func TestConversationLockRegistryEvictsReleasedLocks(t *testing.T) {
	registry := newConversationLockRegistry()
	for index := 0; index < 100; index++ {
		unlock := registry.lock(fmt.Sprintf("missing-%d", index))
		unlock()
	}
	registry.mu.Lock()
	defer registry.mu.Unlock()
	if len(registry.locks) != 0 {
		t.Fatalf("retained locks = %d, want 0", len(registry.locks))
	}
}

func TestCreateConversationReportsStoreLimit(t *testing.T) {
	store := chat.NewMemoryConversationStore()
	h := Handlers{Conversations: store, Model: &fakeModel{}, chatAccessEvaluator: func(context.Context, *auth.User) (bool, error) { return true, nil }}
	for index := 0; index < 100; index++ {
		w := httptest.NewRecorder()
		h.CreateConversation(w, newReq(http.MethodPost, "/api/chat/conversations", "", alice))
		if w.Code != http.StatusCreated {
			t.Fatalf("create %d status = %d; body = %s", index, w.Code, w.Body.String())
		}
	}
	w := httptest.NewRecorder()
	h.CreateConversation(w, newReq(http.MethodPost, "/api/chat/conversations", "", alice))
	if w.Code != http.StatusTooManyRequests {
		t.Fatalf("status = %d, want 429; body = %s", w.Code, w.Body.String())
	}
}

func TestChatEndpointsEnforceEffectiveAccess(t *testing.T) {
	store := chat.NewMemoryConversationStore()
	user := &auth.User{ID: "restricted-user", AZP: "cyclops-cs-spa"}
	tests := []struct {
		name       string
		allowed    bool
		wantStatus int
	}{
		{name: "allowed", allowed: true, wantStatus: http.StatusCreated},
		{name: "denied", allowed: false, wantStatus: http.StatusNotFound},
	}

	for _, test := range tests {
		t.Run(test.name, func(t *testing.T) {
			h := Handlers{
				Conversations: store,
				Model:         &fakeModel{},
				chatAccessEvaluator: func(context.Context, *auth.User) (bool, error) {
					return test.allowed, nil
				},
			}
			w := httptest.NewRecorder()
			h.CreateConversation(w, newReq(http.MethodPost, "/api/chat/conversations", "", user))
			if w.Code != test.wantStatus {
				t.Fatalf("status = %d, want %d; body = %s", w.Code, test.wantStatus, w.Body.String())
			}
		})
	}
}

func TestChatConversationsListFiltersArchived(t *testing.T) {
	h, store, _ := newChatHandlers()
	active := createChatConversation(t, h, alice)
	archived := createChatConversation(t, h, alice)
	if _, err := store.SetArchived(context.Background(), alice.ID, archived.ID, true); err != nil {
		t.Fatalf("archive conversation: %v", err)
	}

	for _, tc := range []struct {
		name       string
		target     string
		statusCode int
		wantID     string
	}{
		{name: "active by default", target: "/api/chat/conversations", statusCode: http.StatusOK, wantID: active.ID},
		{name: "active explicitly", target: "/api/chat/conversations?archived=false", statusCode: http.StatusOK, wantID: active.ID},
		{name: "archived", target: "/api/chat/conversations?archived=true", statusCode: http.StatusOK, wantID: archived.ID},
		{name: "invalid archive value", target: "/api/chat/conversations?archived=1", statusCode: http.StatusBadRequest},
		{name: "present empty archive value", target: "/api/chat/conversations?archived=", statusCode: http.StatusBadRequest},
		{name: "duplicate true archive values", target: "/api/chat/conversations?archived=true&archived=true", statusCode: http.StatusBadRequest},
		{name: "duplicate true and invalid archive values", target: "/api/chat/conversations?archived=true&archived=invalid", statusCode: http.StatusBadRequest},
		{name: "duplicate true and false archive values", target: "/api/chat/conversations?archived=true&archived=false", statusCode: http.StatusBadRequest},
	} {
		t.Run(tc.name, func(t *testing.T) {
			w := httptest.NewRecorder()
			h.ListConversations(w, newReq(http.MethodGet, tc.target, "", alice))
			if w.Code != tc.statusCode {
				t.Fatalf("status = %d, want %d; body = %s", w.Code, tc.statusCode, w.Body.String())
			}
			if tc.wantID == "" {
				return
			}
			var summaries []chat.ConversationSummary
			if err := json.Unmarshal(w.Body.Bytes(), &summaries); err != nil {
				t.Fatalf("decode summaries: %v", err)
			}
			if len(summaries) != 1 || summaries[0].ID != tc.wantID {
				t.Fatalf("summaries = %#v, want %q", summaries, tc.wantID)
			}
		})
	}
}

func TestChatConversationsArchiveAndRestore(t *testing.T) {
	h, _, _ := newChatHandlers()
	conversation := createChatConversation(t, h, alice)

	for _, tc := range []struct {
		name     string
		body     string
		archived bool
	}{
		{name: "archive", body: `{"archived":true}`, archived: true},
		{name: "restore", body: `{"archived":false}`, archived: false},
	} {
		t.Run(tc.name, func(t *testing.T) {
			w := httptest.NewRecorder()
			h.UpdateConversation(w, newReq(http.MethodPatch, "/api/chat/conversations/"+conversation.ID, tc.body, alice))
			if w.Code != http.StatusOK {
				t.Fatalf("status = %d, want 200; body = %s", w.Code, w.Body.String())
			}
			var updated chat.Conversation
			if err := json.Unmarshal(w.Body.Bytes(), &updated); err != nil {
				t.Fatalf("decode conversation: %v", err)
			}
			if (updated.ArchivedAt != nil) != tc.archived {
				t.Fatalf("archived_at = %v, want archived = %t", updated.ArchivedAt, tc.archived)
			}
		})
	}
}

func TestChatConversationsArchiveRejectsInvalidRequests(t *testing.T) {
	h, _, _ := newChatHandlers()
	conversation := createChatConversation(t, h, alice)

	for _, tc := range []struct {
		name       string
		body       string
		statusCode int
	}{
		{name: "missing archived", body: `{}`, statusCode: http.StatusBadRequest},
		{name: "malformed JSON", body: `{"archived":`, statusCode: http.StatusBadRequest},
		{name: "unknown field", body: `{"archived":true,"unexpected":true}`, statusCode: http.StatusBadRequest},
		{name: "trailing JSON", body: `{"archived":true} {}`, statusCode: http.StatusBadRequest},
		{name: "oversized", body: `{"archived":true,"padding":"` + strings.Repeat("x", 4096) + `"}`, statusCode: http.StatusRequestEntityTooLarge},
	} {
		t.Run(tc.name, func(t *testing.T) {
			w := httptest.NewRecorder()
			h.UpdateConversation(w, newReq(http.MethodPatch, "/api/chat/conversations/"+conversation.ID, tc.body, alice))
			if w.Code != tc.statusCode {
				t.Fatalf("status = %d, want %d; body = %s", w.Code, tc.statusCode, w.Body.String())
			}
		})
	}
}

func TestChatConversationsArchiveHidesMissingAndOtherOwners(t *testing.T) {
	h, _, _ := newChatHandlers()
	conversation := createChatConversation(t, h, alice)

	for _, tc := range []struct {
		name string
		id   string
		user *auth.User
	}{
		{name: "missing", id: "missing", user: alice},
		{name: "other owner", id: conversation.ID, user: &auth.User{ID: "user-bob"}},
	} {
		t.Run(tc.name, func(t *testing.T) {
			w := httptest.NewRecorder()
			h.UpdateConversation(w, newReq(http.MethodPatch, "/api/chat/conversations/"+tc.id, `{"archived":true}`, tc.user))
			if w.Code != http.StatusNotFound {
				t.Fatalf("status = %d, want 404; body = %s", w.Code, w.Body.String())
			}
		})
	}
}

func TestChatTurnRejectsArchivedConversation(t *testing.T) {
	h, store, _ := newChatHandlers(chat.Message{Role: chat.RoleAssistant, Content: "unused"})
	conversation := createChatConversation(t, h, alice)
	if _, err := store.SetArchived(context.Background(), alice.ID, conversation.ID, true); err != nil {
		t.Fatalf("archive conversation: %v", err)
	}

	w := httptest.NewRecorder()
	h.CreateTurn(w, newReq(http.MethodPost, "/api/chat/conversations/"+conversation.ID+"/turns", `{"messages":[{"role":"user","content":"hello"}]}`, alice))
	if w.Code != http.StatusConflict {
		t.Fatalf("status = %d, want 409; body = %s", w.Code, w.Body.String())
	}
}
