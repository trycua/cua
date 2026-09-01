package chat

import (
	"bufio"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net/http"
	"sort"
	"strings"
	"time"
)

const (
	bashToolDescription     = "Execute a command in a temporary, isolated in-browser virtual filesystem. Network and host filesystem access are unavailable."
	liteLLMResponseMaxBytes = 128 << 10
	cyclopsSystemPrompt     = `You are the Cyclops CS assistant. Treat unqualified product terms as Cyclops concepts: pools are Cyclops sandbox warm pools, claims are allocated sandboxes, and namespaces scope those resources.

Use browser Bash when it helps. Ordinary Bash has an isolated in-memory filesystem with no network access and no host filesystem access. Registered commands are the only bridges to authenticated Cyclops APIs and the read-only CUA documentation and code service.

Authenticated Cyclops SDK commands:
listNamespaces, listPools, getPool, createPool, updatePoolServices, deletePool, listClaims, createClaim, getClaim, deleteClaim, listUserKeys, createUserKey, deleteUserKey

Read-only CUA documentation and versioned code commands:
query_docs_db, query_docs_vectors, query_code_db, query_code_vectors

You must, before first use of any registered command in a conversation, run exactly <command> -h in a separate Bash tool call and read its full help. The help is that command's skill: follow its Usage, Arguments, Output, Present to the user, Safety, and Examples guidance. After learning a command, do not repeat its help in the same conversation unless you need a refresher. Never guess a command's arguments, behavior, safety, or output presentation instead of reading its help.

Commands write JSON results to stdout, write errors to stderr, and compose with jq, pipes, redirects, files, conditionals, and loops. Use SDK commands for live Cyclops state rather than guessing. Use documentation and code commands for CUA reference questions. Do not claim ordinary Bash can reach APIs or arbitrary network services. Before a mutating command, state the intended change and use the exact values supplied by the user; ask only for missing required values. Be concise and action-oriented; ask a clarifying question only when the Cyclops interpretation is genuinely ambiguous.`
)

// ModelClient produces one assistant response while streaming content deltas.
type ModelClient interface {
	Complete(ctx context.Context, messages []Message, onDelta func(string) error) (Message, error)
}

type LiteLLMClient struct {
	BaseURL    string
	APIKey     string
	Model      string
	HTTPClient *http.Client
}

func NewLiteLLMClient(baseURL, apiKey, model string) *LiteLLMClient {
	return &LiteLLMClient{
		BaseURL:    baseURL,
		APIKey:     apiKey,
		Model:      model,
		HTTPClient: http.DefaultClient,
	}
}

type liteLLMRequest struct {
	Model    string           `json:"model"`
	Stream   bool             `json:"stream"`
	Messages []liteLLMMessage `json:"messages"`
	Tools    []liteLLMTool    `json:"tools"`
}

type liteLLMMessage struct {
	Role       Role       `json:"role"`
	Content    string     `json:"content"`
	ToolCallID string     `json:"tool_call_id,omitempty"`
	ToolCalls  []ToolCall `json:"tool_calls,omitempty"`
}

type liteLLMTool struct {
	Type     string              `json:"type"`
	Function liteLLMToolFunction `json:"function"`
}

type liteLLMToolFunction struct {
	Name        string                `json:"name"`
	Description string                `json:"description"`
	Parameters  liteLLMToolParameters `json:"parameters"`
}

type liteLLMToolParameters struct {
	Type                 string                         `json:"type"`
	AdditionalProperties bool                           `json:"additionalProperties"`
	Properties           map[string]liteLLMToolProperty `json:"properties"`
	Required             []string                       `json:"required"`
}

type liteLLMToolProperty struct {
	Type    string `json:"type"`
	Minimum *int   `json:"minimum,omitempty"`
	Maximum *int   `json:"maximum,omitempty"`
}

type liteLLMChunk struct {
	Choices []liteLLMChoice `json:"choices"`
}

type liteLLMChoice struct {
	Delta *liteLLMDelta `json:"delta"`
}

type liteLLMDelta struct {
	Content   string                 `json:"content"`
	ToolCalls []liteLLMToolCallDelta `json:"tool_calls"`
}

type liteLLMToolCallDelta struct {
	Index    int                  `json:"index"`
	ID       string               `json:"id"`
	Type     string               `json:"type"`
	Function liteLLMFunctionDelta `json:"function"`
}

type liteLLMFunctionDelta struct {
	Name      string `json:"name"`
	Arguments string `json:"arguments"`
}

func (client *LiteLLMClient) Complete(ctx context.Context, messages []Message, onDelta func(string) error) (Message, error) {
	requestBody, err := json.Marshal(liteLLMRequest{
		Model:    client.Model,
		Stream:   true,
		Messages: liteLLMMessages(messages),
		Tools:    []liteLLMTool{bashTool()},
	})
	if err != nil {
		return Message{}, fmt.Errorf("marshal LiteLLM request: %w", err)
	}

	request, err := http.NewRequestWithContext(ctx, http.MethodPost, strings.TrimRight(client.BaseURL, "/")+"/chat/completions", strings.NewReader(string(requestBody)))
	if err != nil {
		return Message{}, fmt.Errorf("create LiteLLM request: %w", err)
	}
	request.Header.Set("Content-Type", "application/json")
	if client.APIKey != "" {
		request.Header.Set("Authorization", "Bearer "+client.APIKey)
	}

	httpClient := client.HTTPClient
	if httpClient == nil {
		httpClient = http.DefaultClient
	}
	response, err := httpClient.Do(request)
	if err != nil {
		if ctxErr := ctx.Err(); ctxErr != nil {
			return Message{}, errors.Join(ctxErr, err)
		}
		return Message{}, fmt.Errorf("send LiteLLM request: %w", err)
	}
	defer response.Body.Close()

	if response.StatusCode < http.StatusOK || response.StatusCode >= http.StatusMultipleChoices {
		return Message{}, liteLLMResponseError(response)
	}

	var content strings.Builder
	toolCalls := make(map[int]ToolCall)
	done := false
	var streamErrors error
	reader := bufio.NewReader(response.Body)
	for {
		data, err := readSSEData(reader)
		if err != nil {
			if errors.Is(err, io.EOF) {
				streamErrors = errors.Join(streamErrors, err)
				break
			}
			if ctxErr := ctx.Err(); ctxErr != nil {
				return Message{}, errors.Join(ctxErr, err, streamErrors)
			}
			return Message{}, errors.Join(fmt.Errorf("read LiteLLM stream: %w", err), streamErrors)
		}
		if err := ctx.Err(); err != nil {
			return Message{}, err
		}
		if strings.TrimSpace(data) == "[DONE]" {
			done = true
			break
		}

		var chunk liteLLMChunk
		if err := json.Unmarshal([]byte(data), &chunk); err != nil {
			return Message{}, fmt.Errorf("parse SSE chunk: %w", err)
		}
		if len(chunk.Choices) != 1 || chunk.Choices[0].Delta == nil {
			return Message{}, errors.New("unsupported LiteLLM SSE response shape")
		}
		if err := ctx.Err(); err != nil {
			return Message{}, err
		}

		delta := chunk.Choices[0].Delta
		if delta.Content != "" {
			if content.Len()+len(delta.Content) > liteLLMResponseMaxBytes {
				return Message{}, errors.New("LiteLLM response exceeds size limit")
			}
			if onDelta != nil {
				if err := onDelta(delta.Content); err != nil {
					return Message{}, fmt.Errorf("handle content delta: %w", err)
				}
				if err := ctx.Err(); err != nil {
					return Message{}, err
				}
			}
			content.WriteString(delta.Content)
		}
		for _, toolCallDelta := range delta.ToolCalls {
			if toolCallDelta.Index < 0 {
				return Message{}, errors.New("unsupported LiteLLM tool call index")
			}
			toolCall := toolCalls[toolCallDelta.Index]
			if toolCallDelta.ID != "" {
				toolCall.ID = toolCallDelta.ID
			}
			if toolCallDelta.Type != "" {
				toolCall.Type = toolCallDelta.Type
			}
			if toolCallDelta.Function.Name != "" {
				toolCall.Function.Name = toolCallDelta.Function.Name
			}
			toolCall.Function.Arguments += toolCallDelta.Function.Arguments
			toolCalls[toolCallDelta.Index] = toolCall
			if content.Len()+toolCallsSize(toolCalls) > liteLLMResponseMaxBytes {
				return Message{}, errors.New("LiteLLM response exceeds size limit")
			}
		}
	}
	if !done {
		return Message{}, errors.Join(errors.New("LiteLLM stream ended without [DONE]"), streamErrors)

	}

	id, err := newUUID()
	if err != nil {
		return Message{}, errors.Join(fmt.Errorf("generate assistant message ID: %w", err), streamErrors)

	}
	return Message{
		ID:        id,
		Role:      RoleAssistant,
		Content:   content.String(),
		ToolCalls: orderedToolCalls(toolCalls),
		CreatedAt: time.Now().UTC(),
	}, nil
}

func toolCallsSize(toolCalls map[int]ToolCall) int {
	total := 0
	for _, call := range toolCalls {
		total += len(call.ID) + len(call.Type) + len(call.Function.Name) + len(call.Function.Arguments)
	}
	return total
}

func liteLLMMessages(messages []Message) []liteLLMMessage {
	mapped := make([]liteLLMMessage, 0, len(messages)+1)
	mapped = append(mapped, liteLLMMessage{Role: Role("system"), Content: cyclopsSystemPrompt})
	for _, message := range messages {
		switch message.Role {
		case RoleUser, RoleAssistant, RoleTool:
			mapped = append(mapped, liteLLMMessage{
				Role:       message.Role,
				Content:    message.Content,
				ToolCallID: message.ToolCallID,
				ToolCalls:  message.ToolCalls,
			})
		}
	}
	return mapped
}

func bashTool() liteLLMTool {
	timeoutMinimum, timeoutMaximum := 250, 60000
	outputMinimum, outputMaximum := 256, 100000
	return liteLLMTool{
		Type: "function",
		Function: liteLLMToolFunction{
			Name:        "bash",
			Description: bashToolDescription,
			Parameters: liteLLMToolParameters{
				Type:                 "object",
				AdditionalProperties: false,
				Properties: map[string]liteLLMToolProperty{
					"command":          {Type: "string"},
					"timeout_ms":       {Type: "integer", Minimum: &timeoutMinimum, Maximum: &timeoutMaximum},
					"max_output_chars": {Type: "integer", Minimum: &outputMinimum, Maximum: &outputMaximum},
				},
				Required: []string{"command"},
			},
		},
	}
}

func readSSEData(reader *bufio.Reader) (string, error) {
	var dataLines []string
	for {
		line, err := reader.ReadString('\n')
		if len(line) > 0 {
			line = strings.TrimSuffix(line, "\n")
			line = strings.TrimSuffix(line, "\r")
			if line == "" {
				if len(dataLines) > 0 {
					return strings.Join(dataLines, "\n"), nil
				}
			} else if strings.HasPrefix(line, "data:") {
				data := strings.TrimPrefix(line, "data:")
				dataLines = append(dataLines, strings.TrimPrefix(data, " "))
			}
		}
		if err != nil {
			if errors.Is(err, io.EOF) && len(dataLines) > 0 {
				return strings.Join(dataLines, "\n"), nil
			}
			return "", err
		}
	}
}

func orderedToolCalls(toolCalls map[int]ToolCall) []ToolCall {
	if len(toolCalls) == 0 {
		return nil
	}
	indexes := make([]int, 0, len(toolCalls))
	for index := range toolCalls {
		indexes = append(indexes, index)
	}
	sort.Ints(indexes)
	ordered := make([]ToolCall, 0, len(indexes))
	for _, index := range indexes {
		ordered = append(ordered, toolCalls[index])
	}
	return ordered
}

func liteLLMResponseError(response *http.Response) error {
	body, err := io.ReadAll(io.LimitReader(response.Body, 1<<20))
	if err != nil {
		return fmt.Errorf("read LiteLLM error response: %w", err)
	}

	message := strings.TrimSpace(string(body))
	var parseErrors error
	var payload struct {
		Error json.RawMessage `json:"error"`
	}
	payloadErr := json.Unmarshal(body, &payload)
	if payloadErr == nil && len(payload.Error) > 0 {
		var detail struct {
			Message string `json:"message"`
		}
		detailErr := json.Unmarshal(payload.Error, &detail)
		if detailErr == nil && detail.Message != "" {
			message = detail.Message
		} else {
			parseErrors = errors.Join(parseErrors, detailErr)
			var errorString string
			stringErr := json.Unmarshal(payload.Error, &errorString)
			if stringErr == nil && errorString != "" {
				message = errorString
			} else {
				parseErrors = errors.Join(parseErrors, stringErr)
			}
		}
	} else {
		parseErrors = errors.Join(parseErrors, payloadErr)
	}
	if message == "" {
		message = response.Status
	}
	return errors.Join(fmt.Errorf("LiteLLM request failed: status %d: %s", response.StatusCode, message), parseErrors)
}
