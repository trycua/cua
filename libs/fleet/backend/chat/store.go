package chat

import (
	"context"
	"crypto/rand"
	"errors"
	"fmt"
	"sort"
	"strings"
	"sync"
	"time"
)

var (
	ErrConversationNotFound = errors.New("conversation not found")
	ErrConversationLimit    = errors.New("conversation storage limit reached")
	ErrConversationArchived = errors.New("conversation is archived")
)

const (
	memoryConversationMaxPerOwner = 100
	memoryConversationMaxBytes    = 64 << 20
)

type ConversationStore interface {
	Create(ctx context.Context, ownerID string) (*Conversation, error)
	List(ctx context.Context, ownerID string, archived bool) ([]ConversationSummary, error)
	Get(ctx context.Context, ownerID, conversationID string) (*Conversation, error)
	SetArchived(ctx context.Context, ownerID, conversationID string, archived bool) (*Conversation, error)
	Append(ctx context.Context, ownerID, conversationID string, messages ...Message) error
}

type conversationRecord struct {
	OwnerID string
	Conversation
}

type MemoryConversationStore struct {
	mu                 sync.RWMutex
	conversations      map[string]*conversationRecord
	ownerConversations map[string]int
	maxPerOwner        int
	maxBytes           int
	totalBytes         int
}

func NewMemoryConversationStore() *MemoryConversationStore {
	return newMemoryConversationStore(memoryConversationMaxPerOwner, memoryConversationMaxBytes)
}

func newMemoryConversationStore(maxPerOwner, maxBytes int) *MemoryConversationStore {
	return &MemoryConversationStore{
		conversations:      make(map[string]*conversationRecord),
		ownerConversations: make(map[string]int),
		maxPerOwner:        maxPerOwner,
		maxBytes:           maxBytes,
	}
}

func (store *MemoryConversationStore) Create(_ context.Context, ownerID string) (*Conversation, error) {
	store.mu.Lock()
	defer store.mu.Unlock()

	if store.ownerConversations[ownerID] >= store.maxPerOwner {
		return nil, ErrConversationLimit
	}
	id, err := newUUID()
	if err != nil {
		return nil, fmt.Errorf("generate conversation ID: %w", err)
	}
	now := time.Now().UTC()
	record := &conversationRecord{
		OwnerID: ownerID,
		Conversation: Conversation{
			ID:        id,
			CreatedAt: now,
			UpdatedAt: now,
		},
	}
	store.conversations[id] = record
	store.ownerConversations[ownerID]++

	return cloneConversation(record.Conversation), nil
}

func (store *MemoryConversationStore) List(_ context.Context, ownerID string, archived bool) ([]ConversationSummary, error) {
	store.mu.RLock()
	summaries := make([]ConversationSummary, 0)
	for _, record := range store.conversations {
		if record.OwnerID != ownerID || (record.ArchivedAt != nil) != archived {
			continue
		}
		summaries = append(summaries, ConversationSummary{
			ID:         record.ID,
			Title:      record.Title,
			CreatedAt:  record.CreatedAt,
			UpdatedAt:  record.UpdatedAt,
			ArchivedAt: cloneTime(record.ArchivedAt),
		})
	}
	store.mu.RUnlock()

	sort.Slice(summaries, func(left, right int) bool {
		if archived {
			return summaries[left].ArchivedAt.After(*summaries[right].ArchivedAt)
		}
		return summaries[left].UpdatedAt.After(summaries[right].UpdatedAt)
	})
	return summaries, nil
}

func (store *MemoryConversationStore) Get(_ context.Context, ownerID, conversationID string) (*Conversation, error) {
	store.mu.RLock()
	record, ok := store.conversations[conversationID]
	if !ok || record.OwnerID != ownerID {
		store.mu.RUnlock()
		return nil, ErrConversationNotFound
	}
	conversation := cloneConversation(record.Conversation)
	store.mu.RUnlock()

	return conversation, nil
}

func (store *MemoryConversationStore) SetArchived(_ context.Context, ownerID, conversationID string, archived bool) (*Conversation, error) {
	store.mu.Lock()
	defer store.mu.Unlock()

	record, ok := store.conversations[conversationID]
	if !ok || record.OwnerID != ownerID {
		return nil, ErrConversationNotFound
	}

	now := time.Now().UTC()
	if archived {
		record.ArchivedAt = &now
	} else {
		record.ArchivedAt = nil
	}
	record.UpdatedAt = now

	return cloneConversation(record.Conversation), nil
}

func (store *MemoryConversationStore) Append(_ context.Context, ownerID, conversationID string, messages ...Message) error {
	store.mu.Lock()
	defer store.mu.Unlock()

	record, ok := store.conversations[conversationID]
	if !ok || record.OwnerID != ownerID {
		return ErrConversationNotFound
	}
	if record.ArchivedAt != nil {
		return ErrConversationArchived
	}
	if len(messages) == 0 {
		return nil
	}

	now := time.Now().UTC()
	pending := make([]Message, len(messages))
	addedBytes := 0
	for index, message := range messages {
		if message.ID == "" {
			id, err := newUUID()
			if err != nil {
				return fmt.Errorf("generate message ID: %w", err)
			}
			message.ID = id
		}
		if message.CreatedAt.IsZero() {
			message.CreatedAt = now
		}
		pending[index] = cloneMessage(message)
		addedBytes += messageSize(message)
	}
	if store.totalBytes+addedBytes > store.maxBytes {
		return ErrConversationLimit
	}
	record.Messages = append(record.Messages, pending...)
	store.totalBytes += addedBytes
	if record.Title == "" {
		record.Title = titleFromFirstUserMessage(record.Messages)
	}
	record.UpdatedAt = now

	return nil
}

func messageSize(message Message) int {
	total := len(message.ID) + len(message.Role) + len(message.Content) + len(message.ToolCallID)
	for _, call := range message.ToolCalls {
		total += len(call.ID) + len(call.Type) + len(call.Function.Name) + len(call.Function.Arguments)
	}
	return total
}

func newUUID() (string, error) {
	bytes := make([]byte, 16)
	if _, err := rand.Read(bytes); err != nil {
		return "", err
	}
	bytes[6] = bytes[6]&0x0f | 0x40
	bytes[8] = bytes[8]&0x3f | 0x80
	return fmt.Sprintf("%08x-%04x-%04x-%04x-%012x", bytes[0:4], bytes[4:6], bytes[6:8], bytes[8:10], bytes[10:16]), nil
}

func titleFromFirstUserMessage(messages []Message) string {
	for _, message := range messages {
		if message.Role != RoleUser {
			continue
		}
		title := strings.TrimSpace(message.Content)
		runes := []rune(title)
		if len(runes) > 80 {
			return string(runes[:80])
		}
		return title
	}
	return ""
}

func cloneConversation(conversation Conversation) *Conversation {
	cloned := conversation
	cloned.ArchivedAt = cloneTime(conversation.ArchivedAt)
	cloned.Messages = make([]Message, len(conversation.Messages))
	for index, message := range conversation.Messages {
		cloned.Messages[index] = cloneMessage(message)
	}
	return &cloned
}

func cloneMessage(message Message) Message {
	cloned := message
	cloned.ToolCalls = append([]ToolCall(nil), message.ToolCalls...)
	return cloned
}

func cloneTime(value *time.Time) *time.Time {
	if value == nil {
		return nil
	}
	cloned := *value
	return &cloned
}
