package chat

import (
	"context"
	"errors"
	"fmt"
	"strings"
	"sync"
	"testing"
	"time"
)

func TestMemoryStoreTitlesAndOrdersConversations(t *testing.T) {
	store := NewMemoryConversationStore()
	first, err := store.Create(context.Background(), "owner-a")
	if err != nil {
		t.Fatal(err)
	}
	second, err := store.Create(context.Background(), "owner-a")
	if err != nil {
		t.Fatal(err)
	}
	if err := store.Append(context.Background(), "owner-a", first.ID, Message{Role: RoleUser, Content: "  First useful question  "}); err != nil {
		t.Fatal(err)
	}
	if err := store.Append(context.Background(), "owner-a", second.ID, Message{Role: RoleUser, Content: "Newest question"}); err != nil {
		t.Fatal(err)
	}

	got, err := store.List(context.Background(), "owner-a", false)
	if err != nil {
		t.Fatal(err)
	}
	if len(got) != 2 || got[0].ID != second.ID || got[1].Title != "First useful question" {
		t.Fatalf("unexpected summaries: %#v", got)
	}
}

func TestMemoryConversationStoreArchivesAndRestores(t *testing.T) {
	store := NewMemoryConversationStore()
	conversation, err := store.Create(context.Background(), "owner-a")
	if err != nil {
		t.Fatal(err)
	}

	archived, err := store.SetArchived(context.Background(), "owner-a", conversation.ID, true)
	if err != nil {
		t.Fatal(err)
	}
	if archived.ArchivedAt == nil {
		t.Fatalf("archive = %#v, want archived timestamp", archived)
	}
	if !archived.UpdatedAt.After(conversation.UpdatedAt) {
		t.Fatalf("archive UpdatedAt = %v, creation UpdatedAt = %v", archived.UpdatedAt, conversation.UpdatedAt)
	}
	if !archived.UpdatedAt.Equal(*archived.ArchivedAt) {
		t.Fatalf("archive UpdatedAt = %v, ArchivedAt = %v", archived.UpdatedAt, archived.ArchivedAt)
	}
	if archived.UpdatedAt.Location() != time.UTC || archived.ArchivedAt.Location() != time.UTC {
		t.Fatalf("archive timestamps must be UTC: UpdatedAt=%v ArchivedAt=%v", archived.UpdatedAt.Location(), archived.ArchivedAt.Location())
	}
	archiveTimestamp := *archived.ArchivedAt
	*archived.ArchivedAt = time.Time{}

	stored, err := store.Get(context.Background(), "owner-a", conversation.ID)
	if err != nil {
		t.Fatal(err)
	}
	if stored.ArchivedAt == nil || stored.ArchivedAt.IsZero() {
		t.Fatalf("stored archive timestamp = %v, want non-zero", stored.ArchivedAt)
	}
	if !stored.ArchivedAt.Equal(archiveTimestamp) {
		t.Fatalf("stored archive timestamp = %v, want %v", stored.ArchivedAt, archiveTimestamp)
	}

	active, err := store.List(context.Background(), "owner-a", false)
	if err != nil {
		t.Fatal(err)
	}
	archivedSummaries, err := store.List(context.Background(), "owner-a", true)
	if err != nil {
		t.Fatal(err)
	}
	if len(active) != 0 || len(archivedSummaries) != 1 {
		t.Fatalf("active=%#v archived=%#v", active, archivedSummaries)
	}
	if archivedSummaries[0].ArchivedAt == nil || archivedSummaries[0].ArchivedAt.IsZero() {
		t.Fatalf("archived summary timestamp = %v, want non-zero", archivedSummaries[0].ArchivedAt)
	}

	restored, err := store.SetArchived(context.Background(), "owner-a", conversation.ID, false)
	if err != nil {
		t.Fatal(err)
	}
	if restored.ArchivedAt != nil {
		t.Fatalf("restored archive timestamp = %v, want nil", restored.ArchivedAt)
	}
	if !restored.UpdatedAt.After(archiveTimestamp) {
		t.Fatalf("restored UpdatedAt = %v, archive timestamp = %v", restored.UpdatedAt, archiveTimestamp)
	}
	if restored.UpdatedAt.Location() != time.UTC {
		t.Fatalf("restored UpdatedAt location = %v, want UTC", restored.UpdatedAt.Location())
	}

	active, err = store.List(context.Background(), "owner-a", false)
	if err != nil {
		t.Fatal(err)
	}
	archivedSummaries, err = store.List(context.Background(), "owner-a", true)
	if err != nil {
		t.Fatal(err)
	}
	if len(active) != 1 || active[0].ID != conversation.ID || len(archivedSummaries) != 0 {
		t.Fatalf("restored active=%#v archived=%#v", active, archivedSummaries)
	}
}

func TestMemoryConversationStoreFiltersArchivedByOwner(t *testing.T) {
	store := NewMemoryConversationStore()
	active, err := store.Create(context.Background(), "owner-a")
	if err != nil {
		t.Fatal(err)
	}
	firstArchived, err := store.Create(context.Background(), "owner-a")
	if err != nil {
		t.Fatal(err)
	}
	secondArchived, err := store.Create(context.Background(), "owner-a")
	if err != nil {
		t.Fatal(err)
	}
	otherOwner, err := store.Create(context.Background(), "owner-b")
	if err != nil {
		t.Fatal(err)
	}
	if _, err := store.SetArchived(context.Background(), "owner-a", firstArchived.ID, true); err != nil {
		t.Fatal(err)
	}
	if _, err := store.SetArchived(context.Background(), "owner-a", secondArchived.ID, true); err != nil {
		t.Fatal(err)
	}
	if _, err := store.SetArchived(context.Background(), "owner-b", otherOwner.ID, true); err != nil {
		t.Fatal(err)
	}
	if _, err := store.SetArchived(context.Background(), "owner-b", firstArchived.ID, false); !errors.Is(err, ErrConversationNotFound) {
		t.Fatalf("SetArchived error = %v, want ErrConversationNotFound", err)
	}

	activeSummaries, err := store.List(context.Background(), "owner-a", false)
	if err != nil {
		t.Fatal(err)
	}
	if len(activeSummaries) != 1 || activeSummaries[0].ID != active.ID {
		t.Fatalf("active summaries = %#v", activeSummaries)
	}

	archivedSummaries, err := store.List(context.Background(), "owner-a", true)
	if err != nil {
		t.Fatal(err)
	}
	if len(archivedSummaries) != 2 || archivedSummaries[0].ID != secondArchived.ID || archivedSummaries[1].ID != firstArchived.ID {
		t.Fatalf("archived summaries = %#v", archivedSummaries)
	}
	for _, summary := range archivedSummaries {
		if summary.ArchivedAt == nil {
			t.Fatalf("archived summary = %#v, want archived timestamp", summary)
		}
	}
}

func TestMemoryConversationStoreRejectsAppendToArchived(t *testing.T) {
	store := NewMemoryConversationStore()
	conversation, err := store.Create(context.Background(), "owner-a")
	if err != nil {
		t.Fatal(err)
	}
	if _, err := store.SetArchived(context.Background(), "owner-a", conversation.ID, true); err != nil {
		t.Fatal(err)
	}

	err = store.Append(context.Background(), "owner-a", conversation.ID, Message{Role: RoleUser, Content: "blocked"})
	if !errors.Is(err, ErrConversationArchived) {
		t.Fatalf("Append error = %v, want ErrConversationArchived", err)
	}
}

func TestMemoryStoreHidesOtherOwners(t *testing.T) {
	store := NewMemoryConversationStore()
	conversation, err := store.Create(context.Background(), "owner-a")
	if err != nil {
		t.Fatal(err)
	}
	if _, err := store.Get(context.Background(), "owner-b", conversation.ID); !errors.Is(err, ErrConversationNotFound) {
		t.Fatalf("Get error = %v, want ErrConversationNotFound", err)
	}
	if err := store.Append(context.Background(), "owner-b", conversation.ID, Message{Role: RoleUser, Content: "no"}); !errors.Is(err, ErrConversationNotFound) {
		t.Fatalf("Append error = %v, want ErrConversationNotFound", err)
	}
}

func TestMemoryStoreReturnsDeepCopies(t *testing.T) {
	store := NewMemoryConversationStore()
	conversation, err := store.Create(context.Background(), "owner-a")
	if err != nil {
		t.Fatal(err)
	}
	message := Message{
		Role:    RoleAssistant,
		Content: "original",
		ToolCalls: []ToolCall{{
			ID:   "call-1",
			Type: "function",
			Function: ToolFunction{
				Name:      "run_bash",
				Arguments: `{"command":"pwd"}`,
			},
		}},
	}
	if err := store.Append(context.Background(), "owner-a", conversation.ID, message); err != nil {
		t.Fatal(err)
	}

	got, err := store.Get(context.Background(), "owner-a", conversation.ID)
	if err != nil {
		t.Fatal(err)
	}
	got.Title = "mutated"
	got.Messages[0].Content = "mutated"
	got.Messages[0].ToolCalls[0].Function.Name = "mutated"

	again, err := store.Get(context.Background(), "owner-a", conversation.ID)
	if err != nil {
		t.Fatal(err)
	}
	if again.Title != "" || again.Messages[0].Content != "original" || again.Messages[0].ToolCalls[0].Function.Name != "run_bash" {
		t.Fatalf("store leaked returned mutation: %#v", again)
	}
}

func TestMemoryStoreConcurrentAppend(t *testing.T) {
	store := NewMemoryConversationStore()
	conversation, err := store.Create(context.Background(), "owner-a")
	if err != nil {
		t.Fatal(err)
	}

	const writers = 25
	var waitGroup sync.WaitGroup
	waitGroup.Add(writers)
	for index := range writers {
		go func() {
			defer waitGroup.Done()
			if err := store.Append(context.Background(), "owner-a", conversation.ID, Message{Role: RoleUser, Content: fmt.Sprintf("message-%d", index)}); err != nil {
				t.Errorf("Append() error = %v", err)
			}
		}()
	}
	waitGroup.Wait()

	got, err := store.Get(context.Background(), "owner-a", conversation.ID)
	if err != nil {
		t.Fatal(err)
	}
	if len(got.Messages) != writers {
		t.Fatalf("message count = %d, want %d", len(got.Messages), writers)
	}
}

func TestMemoryStoreCreateDoesNotRaceWithDiscoveryAndAppend(t *testing.T) {
	const (
		creators                = 4
		conversationsPerCreator = 1_000
	)

	store := newMemoryConversationStore(creators*conversationsPerCreator, memoryConversationMaxBytes)
	ctx := context.Background()
	done := make(chan struct{})
	var discoveryWaitGroup sync.WaitGroup
	discoveryWaitGroup.Add(1)
	go func() {
		defer discoveryWaitGroup.Done()
		seen := make(map[string]struct{})
		for {
			select {
			case <-done:
				return
			default:
			}

			summaries, err := store.List(ctx, "owner-a", false)
			if err != nil {
				t.Errorf("List() error = %v", err)
				return
			}
			for _, summary := range summaries {
				if _, ok := seen[summary.ID]; ok {
					continue
				}
				seen[summary.ID] = struct{}{}
				if err := store.Append(ctx, "owner-a", summary.ID, Message{Role: RoleUser, Content: "discovered"}); err != nil {
					t.Errorf("Append() error = %v", err)
					return
				}
			}
		}
	}()

	var createWaitGroup sync.WaitGroup
	createWaitGroup.Add(creators)
	for range creators {
		go func() {
			defer createWaitGroup.Done()
			for range conversationsPerCreator {
				if _, err := store.Create(ctx, "owner-a"); err != nil {
					t.Errorf("Create() error = %v", err)
					return
				}
			}
		}()
	}
	createWaitGroup.Wait()
	close(done)
	discoveryWaitGroup.Wait()
}

func TestMemoryStoreStampsMessagesAndCapsTitle(t *testing.T) {
	store := NewMemoryConversationStore()
	conversation, err := store.Create(context.Background(), "owner-a")
	if err != nil {
		t.Fatal(err)
	}
	content := "  " + strings.Repeat("a", 81) + "  "
	if err := store.Append(context.Background(), "owner-a", conversation.ID, Message{Role: RoleUser, Content: content}); err != nil {
		t.Fatal(err)
	}

	got, err := store.Get(context.Background(), "owner-a", conversation.ID)
	if err != nil {
		t.Fatal(err)
	}
	if got.Title != content[2:82] {
		t.Fatalf("title = %q, want 80-character prefix", got.Title)
	}
	if got.Messages[0].ID == "" {
		t.Fatal("message ID was not stamped")
	}
	if got.Messages[0].CreatedAt.IsZero() {
		t.Fatal("message CreatedAt was not stamped")
	}
	if !got.UpdatedAt.After(got.CreatedAt) && !got.UpdatedAt.Equal(got.CreatedAt) {
		t.Fatalf("UpdatedAt = %v, CreatedAt = %v", got.UpdatedAt, got.CreatedAt)
	}
}

func TestMemoryConversationStoreLimitsConversationsPerOwner(t *testing.T) {
	store := newMemoryConversationStore(2, 1<<20)
	for index := 0; index < 2; index++ {
		if _, err := store.Create(context.Background(), "owner-a"); err != nil {
			t.Fatalf("create %d: %v", index, err)
		}
	}
	if _, err := store.Create(context.Background(), "owner-a"); !errors.Is(err, ErrConversationLimit) {
		t.Fatalf("error = %v, want ErrConversationLimit", err)
	}
	if _, err := store.Create(context.Background(), "owner-b"); err != nil {
		t.Fatalf("other owner create: %v", err)
	}
}

func TestMemoryConversationStoreLimitsAggregateBytes(t *testing.T) {
	store := newMemoryConversationStore(10, 1024)
	conversation, err := store.Create(context.Background(), "owner-a")
	if err != nil {
		t.Fatal(err)
	}
	if err := store.Append(context.Background(), "owner-a", conversation.ID, Message{Role: RoleUser, Content: strings.Repeat("x", 2048)}); !errors.Is(err, ErrConversationLimit) {
		t.Fatalf("error = %v, want ErrConversationLimit", err)
	}
	stored, err := store.Get(context.Background(), "owner-a", conversation.ID)
	if err != nil {
		t.Fatal(err)
	}
	if len(stored.Messages) != 0 {
		t.Fatalf("oversized append mutated conversation: %#v", stored.Messages)
	}
}
