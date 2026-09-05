package chat

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
	"time"
)

func TestLiteLLMCompleteStreamsContentAndRequest(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(writer http.ResponseWriter, request *http.Request) {
		if request.Method != http.MethodPost {
			t.Errorf("method = %s, want POST", request.Method)
		}
		if request.URL.Path != "/v1/chat/completions" {
			t.Errorf("path = %s, want /v1/chat/completions", request.URL.Path)
		}
		if authorization := request.Header.Get("Authorization"); authorization != "Bearer test-key" {
			t.Errorf("Authorization = %q, want bearer token", authorization)
		}

		var body struct {
			Model    string `json:"model"`
			Stream   bool   `json:"stream"`
			Messages []struct {
				Role    string `json:"role"`
				Content string `json:"content"`
			} `json:"messages"`
			Tools []struct {
				Type     string `json:"type"`
				Function struct {
					Name        string `json:"name"`
					Description string `json:"description"`
					Parameters  struct {
						Type                 string `json:"type"`
						AdditionalProperties bool   `json:"additionalProperties"`
						Properties           map[string]struct {
							Type    string `json:"type"`
							Minimum *int   `json:"minimum"`
							Maximum *int   `json:"maximum"`
						} `json:"properties"`
						Required []string `json:"required"`
					} `json:"parameters"`
				} `json:"function"`
			} `json:"tools"`
		}
		if err := json.NewDecoder(request.Body).Decode(&body); err != nil {
			t.Fatal(err)
		}
		if body.Model != "test-model" || !body.Stream {
			t.Errorf("model/stream = %q/%t, want test-model/true", body.Model, body.Stream)
		}
		if len(body.Messages) != 2 {
			t.Fatalf("message count = %d, want system and user", len(body.Messages))
		}
		if body.Messages[0].Role != "system" {
			t.Fatalf("first message role = %q, want system", body.Messages[0].Role)
		}
		for _, fragment := range []string{"Cyclops CS", "listPools", "getPool", "listClaims", "query_docs_db", "query_docs_vectors", "query_code_db", "query_code_vectors", "-h", "before first use", "same conversation", "Present to the user", "no network access"} {
			if !strings.Contains(body.Messages[0].Content, fragment) {
				t.Errorf("system prompt missing %q", fragment)
			}
		}
		if body.Messages[1].Role != "user" || body.Messages[1].Content != "Say hello" {
			t.Errorf("user message = %#v", body.Messages[1])
		}
		if len(body.Tools) != 1 {
			t.Fatalf("tool count = %d, want 1", len(body.Tools))
		}
		tool := body.Tools[0]
		if tool.Type != "function" || tool.Function.Name != "bash" {
			t.Fatalf("tool = %#v, want bash function", tool)
		}
		if tool.Function.Description != "Execute a command in a temporary, isolated in-browser virtual filesystem. Network and host filesystem access are unavailable." {
			t.Errorf("description = %q", tool.Function.Description)
		}
		parameters := tool.Function.Parameters
		if parameters.Type != "object" || parameters.AdditionalProperties {
			t.Errorf("parameters object/additionalProperties = %q/%t", parameters.Type, parameters.AdditionalProperties)
		}
		if got := parameters.Required; len(got) != 1 || got[0] != "command" {
			t.Errorf("required = %#v, want [command]", got)
		}
		if parameters.Properties["command"].Type != "string" || parameters.Properties["timeout_ms"].Minimum == nil || *parameters.Properties["timeout_ms"].Minimum != 250 || parameters.Properties["timeout_ms"].Maximum == nil || *parameters.Properties["timeout_ms"].Maximum != 60000 || parameters.Properties["max_output_chars"].Minimum == nil || *parameters.Properties["max_output_chars"].Minimum != 256 || parameters.Properties["max_output_chars"].Maximum == nil || *parameters.Properties["max_output_chars"].Maximum != 100000 {
			t.Errorf("unexpected bash tool parameters: %#v", parameters)
		}

		writer.Header().Set("Content-Type", "text/event-stream")
		fmt.Fprint(writer, "data: {\"choices\":[{\"delta\":{\"content\":\"Hello \"}}]}\n\n")
		fmt.Fprint(writer, "data: {\"choices\":[{\"delta\":{\"content\":\"world\"}}]}\n\n")
		fmt.Fprint(writer, "data: [DONE]\n\n")
	}))
	defer server.Close()

	client := NewLiteLLMClient(server.URL+"/v1", "test-key", "test-model")
	var deltas []string
	message, err := client.Complete(context.Background(), []Message{
		{Role: Role("system"), Content: "must not be sent"},
		{Role: RoleUser, Content: "Say hello"},
	}, func(delta string) error {
		deltas = append(deltas, delta)
		return nil
	})
	if err != nil {
		t.Fatal(err)
	}
	if message.Role != RoleAssistant || message.Content != "Hello world" {
		t.Fatalf("message = %#v, want assembled assistant content", message)
	}
	if message.ID == "" || message.CreatedAt.IsZero() {
		t.Fatalf("message was not stamped: %#v", message)
	}
	if got := strings.Join(deltas, ""); got != "Hello world" {
		t.Fatalf("deltas = %#v, want Hello world", deltas)
	}
}

func TestLiteLLMCompleteAssemblesIndexedToolCallsAcrossLargeFrames(t *testing.T) {
	largeArguments := strings.Repeat("x", 70_000)
	server := httptest.NewServer(http.HandlerFunc(func(writer http.ResponseWriter, request *http.Request) {
		writer.Header().Set("Content-Type", "text/event-stream")
		fmt.Fprint(writer, "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call-1\",\"type\":\"function\",\"function\":{\"name\":\"bash\",\"arguments\":\"{\\\"command\\\":\\\"p\"}}]}}]}\n\n")
		fmt.Fprintf(writer, "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":%q}}]}}]}\n\n", largeArguments+`wd"}`)
		fmt.Fprint(writer, "data: [DONE]\n\n")
	}))
	defer server.Close()

	message, err := NewLiteLLMClient(server.URL, "", "test-model").Complete(context.Background(), []Message{{Role: RoleUser, Content: "where am I?"}}, nil)
	if err != nil {
		t.Fatal(err)
	}
	if len(message.ToolCalls) != 1 {
		t.Fatalf("tool calls = %#v, want one", message.ToolCalls)
	}
	wantArguments := `{"command":"p` + largeArguments + `wd"}`
	want := ToolCall{ID: "call-1", Type: "function", Function: ToolFunction{Name: "bash", Arguments: wantArguments}}
	if got := message.ToolCalls[0]; got != want {
		t.Fatalf("tool call = %#v, want %#v", got, want)
	}
}

func TestLiteLLMCompleteReturnsNonSuccessJSONError(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(writer http.ResponseWriter, request *http.Request) {
		writer.Header().Set("Content-Type", "application/json")
		writer.WriteHeader(http.StatusTooManyRequests)
		fmt.Fprint(writer, `{"error":{"message":"rate limited"}}`)
	}))
	defer server.Close()

	_, err := NewLiteLLMClient(server.URL, "", "test-model").Complete(context.Background(), nil, nil)
	if err == nil || !strings.Contains(err.Error(), "429") || !strings.Contains(err.Error(), "rate limited") {
		t.Fatalf("error = %v, want status and JSON message", err)
	}
}

func TestLiteLLMCompleteRejectsMalformedSSE(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(writer http.ResponseWriter, request *http.Request) {
		writer.Header().Set("Content-Type", "text/event-stream")
		fmt.Fprint(writer, "data: {not json}\n\n")
	}))
	defer server.Close()

	_, err := NewLiteLLMClient(server.URL, "", "test-model").Complete(context.Background(), nil, nil)
	if err == nil || !strings.Contains(err.Error(), "parse SSE chunk") {
		t.Fatalf("error = %v, want malformed SSE error", err)
	}
}

func TestLiteLLMCompleteRejectsMissingDone(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(writer http.ResponseWriter, request *http.Request) {
		writer.Header().Set("Content-Type", "text/event-stream")
		fmt.Fprint(writer, "data: {\"choices\":[{\"delta\":{\"content\":\"incomplete\"}}]}\n\n")
	}))
	defer server.Close()

	_, err := NewLiteLLMClient(server.URL, "", "test-model").Complete(context.Background(), nil, nil)
	if err == nil || !strings.Contains(err.Error(), "[DONE]") {
		t.Fatalf("error = %v, want missing [DONE] error", err)
	}
}

func TestLiteLLMCompleteReturnsCallbackError(t *testing.T) {
	callbackError := errors.New("client disconnected")
	server := httptest.NewServer(http.HandlerFunc(func(writer http.ResponseWriter, request *http.Request) {
		writer.Header().Set("Content-Type", "text/event-stream")
		fmt.Fprint(writer, "data: {\"choices\":[{\"delta\":{\"content\":\"Hello\"}}]}\n\n")
		fmt.Fprint(writer, "data: [DONE]\n\n")
	}))
	defer server.Close()

	_, err := NewLiteLLMClient(server.URL, "", "test-model").Complete(context.Background(), nil, func(string) error {
		return callbackError
	})
	if !errors.Is(err, callbackError) {
		t.Fatalf("error = %v, want callback error", err)
	}
}

func TestLiteLLMCompleteReturnsContextCancellation(t *testing.T) {
	requestStarted := make(chan struct{})
	server := httptest.NewServer(http.HandlerFunc(func(writer http.ResponseWriter, request *http.Request) {
		writer.Header().Set("Content-Type", "text/event-stream")
		writer.WriteHeader(http.StatusOK)
		writer.(http.Flusher).Flush()
		close(requestStarted)
		<-request.Context().Done()
	}))
	defer server.Close()

	requestContext, cancel := context.WithCancel(context.Background())
	defer cancel()
	errorsChannel := make(chan error, 1)
	go func() {
		_, err := NewLiteLLMClient(server.URL, "", "test-model").Complete(requestContext, nil, nil)
		errorsChannel <- err
	}()

	select {
	case <-requestStarted:
		cancel()
	case <-time.After(time.Second):
		t.Fatal("request did not reach test server")
	}
	select {
	case err := <-errorsChannel:
		if !errors.Is(err, context.Canceled) {
			t.Fatalf("error = %v, want context cancellation", err)
		}
	case <-time.After(time.Second):
		t.Fatal("Complete did not return after context cancellation")
	}
}

func TestLiteLLMCompleteReturnsCancellationFromSuccessfulDeltaCallback(t *testing.T) {
	requestContext, cancel := context.WithCancel(context.Background())
	defer cancel()

	server := httptest.NewServer(http.HandlerFunc(func(writer http.ResponseWriter, request *http.Request) {
		writer.Header().Set("Content-Type", "text/event-stream")
		fmt.Fprint(writer, "data: {\"choices\":[{\"delta\":{\"content\":\"Hello\"}}]}\n\n")
		fmt.Fprint(writer, "data: [DONE]\n\n")
	}))
	defer server.Close()

	message, err := NewLiteLLMClient(server.URL, "", "test-model").Complete(requestContext, nil, func(string) error {
		cancel()
		return nil
	})
	if !errors.Is(err, context.Canceled) {
		t.Fatalf("error = %v, want context cancellation", err)
	}
	if message.ID != "" || message.Content != "" || message.Role != "" {
		t.Fatalf("message = %#v, want zero message after cancellation", message)
	}
}

func TestLiteLLMCompleteRejectsOversizedAssistantOutput(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(writer http.ResponseWriter, request *http.Request) {
		writer.Header().Set("Content-Type", "text/event-stream")
		fmt.Fprintf(writer, "data: {\"choices\":[{\"delta\":{\"content\":%q}}]}\n\n", strings.Repeat("x", (128<<10)+1))
		fmt.Fprint(writer, "data: [DONE]\n\n")
	}))
	defer server.Close()

	client := NewLiteLLMClient(server.URL, "", "test-model")
	_, err := client.Complete(context.Background(), []Message{{Role: RoleUser, Content: "hello"}}, nil)
	if err == nil || !strings.Contains(err.Error(), "response exceeds") {
		t.Fatalf("error = %v, want response size error", err)
	}
}
