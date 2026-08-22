package chat

import (
	"encoding/json"
	"time"
)

type Role string

const (
	RoleUser      Role = "user"
	RoleAssistant Role = "assistant"
	RoleTool      Role = "tool"
)

type ToolFunction struct {
	Name      string `json:"name"`
	Arguments string `json:"arguments"`
}

type ToolCall struct {
	ID       string       `json:"id"`
	Type     string       `json:"type"`
	Function ToolFunction `json:"function"`
}

type Message struct {
	ID         string     `json:"id,omitempty"`
	Role       Role       `json:"role"`
	Content    string     `json:"content"`
	ToolCallID string     `json:"tool_call_id,omitempty"`
	ToolCalls  []ToolCall `json:"tool_calls,omitempty"`
	CreatedAt  time.Time  `json:"created_at,omitempty"`
}

type Conversation struct {
	ID         string     `json:"id"`
	Title      string     `json:"title"`
	Messages   []Message  `json:"messages"`
	CreatedAt  time.Time  `json:"created_at"`
	UpdatedAt  time.Time  `json:"updated_at"`
	ArchivedAt *time.Time `json:"archived_at,omitempty"`
}

type ConversationSummary struct {
	ID         string     `json:"id"`
	Title      string     `json:"title"`
	CreatedAt  time.Time  `json:"created_at"`
	UpdatedAt  time.Time  `json:"updated_at"`
	ArchivedAt *time.Time `json:"archived_at,omitempty"`
}

type messageJSON struct {
	ID         string     `json:"id,omitempty"`
	Role       Role       `json:"role"`
	Content    string     `json:"content"`
	ToolCallID string     `json:"tool_call_id,omitempty"`
	ToolCalls  []ToolCall `json:"tool_calls,omitempty"`
	CreatedAt  *time.Time `json:"created_at,omitempty"`
}

func (message Message) MarshalJSON() ([]byte, error) {
	var createdAt *time.Time
	if !message.CreatedAt.IsZero() {
		createdAt = &message.CreatedAt
	}
	return json.Marshal(messageJSON{
		ID:         message.ID,
		Role:       message.Role,
		Content:    message.Content,
		ToolCallID: message.ToolCallID,
		ToolCalls:  message.ToolCalls,
		CreatedAt:  createdAt,
	})
}
