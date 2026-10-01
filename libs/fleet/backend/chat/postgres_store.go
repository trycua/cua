package chat

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"time"

	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgxpool"
)

const postgresConversationColumns = "id, title, messages, created_at, updated_at, archived_at"

type PostgresConversationStore struct {
	pool *pgxpool.Pool
	now  func() time.Time
}

func NewPostgresConversationStore(ctx context.Context, databaseURL string) (*PostgresConversationStore, error) {
	config, err := pgxpool.ParseConfig(databaseURL)
	if err != nil {
		return nil, fmt.Errorf("parse chat database URL: %w", err)
	}
	pool, err := pgxpool.NewWithConfig(ctx, config)
	if err != nil {
		return nil, fmt.Errorf("create chat database pool: %w", err)
	}
	return &PostgresConversationStore{pool: pool, now: func() time.Time { return time.Now().UTC() }}, nil
}

func (store *PostgresConversationStore) Close() {
	store.pool.Close()
}

func (store *PostgresConversationStore) Create(ctx context.Context, ownerID string) (*Conversation, error) {
	tx, err := store.pool.Begin(ctx)
	if err != nil {
		return nil, fmt.Errorf("begin create conversation: %w", err)
	}
	defer tx.Rollback(ctx)

	if _, err := tx.Exec(ctx, `SELECT pg_advisory_xact_lock(hashtextextended($1, 0))`, ownerID); err != nil {
		return nil, fmt.Errorf("lock conversation owner: %w", err)
	}
	var count int
	if err := tx.QueryRow(ctx, `SELECT count(*) FROM chat_conversations WHERE owner_sub = $1`, ownerID).Scan(&count); err != nil {
		return nil, fmt.Errorf("count owner conversations: %w", err)
	}
	if count >= memoryConversationMaxPerOwner {
		return nil, ErrConversationLimit
	}

	id, err := newUUID()
	if err != nil {
		return nil, fmt.Errorf("generate conversation ID: %w", err)
	}
	now := store.now()
	conversation := &Conversation{ID: id, CreatedAt: now, UpdatedAt: now}
	if _, err := tx.Exec(ctx, `
		INSERT INTO chat_conversations (id, owner_sub, title, messages, message_bytes, created_at, updated_at)
		VALUES ($1, $2, '', '[]'::jsonb, 0, $3, $3)`, id, ownerID, now); err != nil {
		return nil, fmt.Errorf("insert conversation: %w", err)
	}
	if err := tx.Commit(ctx); err != nil {
		return nil, fmt.Errorf("commit create conversation: %w", err)
	}
	return conversation, nil
}

func (store *PostgresConversationStore) List(ctx context.Context, ownerID string, archived bool) ([]ConversationSummary, error) {
	rows, err := store.pool.Query(ctx, `
		SELECT id, title, created_at, updated_at, archived_at
		FROM chat_conversations
		WHERE owner_sub = $1 AND (archived_at IS NOT NULL) = $2
		ORDER BY CASE WHEN $2 THEN archived_at ELSE updated_at END DESC, id`, ownerID, archived)
	if err != nil {
		return nil, fmt.Errorf("list conversations: %w", err)
	}
	defer rows.Close()

	summaries := make([]ConversationSummary, 0)
	for rows.Next() {
		var summary ConversationSummary
		if err := rows.Scan(&summary.ID, &summary.Title, &summary.CreatedAt, &summary.UpdatedAt, &summary.ArchivedAt); err != nil {
			return nil, fmt.Errorf("scan conversation summary: %w", err)
		}
		summaries = append(summaries, summary)
	}
	if err := rows.Err(); err != nil {
		return nil, fmt.Errorf("iterate conversation summaries: %w", err)
	}
	return summaries, nil
}

func (store *PostgresConversationStore) Get(ctx context.Context, ownerID, conversationID string) (*Conversation, error) {
	conversation, err := scanPostgresConversation(store.pool.QueryRow(ctx, `
		SELECT `+postgresConversationColumns+`
		FROM chat_conversations
		WHERE id = $1 AND owner_sub = $2`, conversationID, ownerID))
	if errors.Is(err, pgx.ErrNoRows) {
		return nil, ErrConversationNotFound
	}
	if err != nil {
		return nil, fmt.Errorf("get conversation: %w", err)
	}
	return conversation, nil
}

func (store *PostgresConversationStore) SetArchived(ctx context.Context, ownerID, conversationID string, archived bool) (*Conversation, error) {
	now := store.now()
	conversation, err := scanPostgresConversation(store.pool.QueryRow(ctx, `
		UPDATE chat_conversations
		SET archived_at = CASE WHEN $3 THEN $4::timestamptz ELSE NULL::timestamptz END, updated_at = $4
		WHERE id = $1 AND owner_sub = $2
		RETURNING `+postgresConversationColumns, conversationID, ownerID, archived, now))
	if errors.Is(err, pgx.ErrNoRows) {
		return nil, ErrConversationNotFound
	}
	if err != nil {
		return nil, fmt.Errorf("archive conversation: %w", err)
	}
	return conversation, nil
}

func (store *PostgresConversationStore) Append(ctx context.Context, ownerID, conversationID string, messages ...Message) error {
	if len(messages) == 0 {
		return nil
	}

	now := store.now()
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
	encoded, err := json.Marshal(pending)
	if err != nil {
		return fmt.Errorf("encode chat messages: %w", err)
	}

	tx, err := store.pool.Begin(ctx)
	if err != nil {
		return fmt.Errorf("begin append conversation: %w", err)
	}
	defer tx.Rollback(ctx)

	var archivedAt *time.Time
	var currentBytes int
	var title string
	if err := tx.QueryRow(ctx, `
		SELECT archived_at, message_bytes, title
		FROM chat_conversations
		WHERE id = $1 AND owner_sub = $2
		FOR UPDATE`, conversationID, ownerID).Scan(&archivedAt, &currentBytes, &title); errors.Is(err, pgx.ErrNoRows) {
		return ErrConversationNotFound
	} else if err != nil {
		return fmt.Errorf("lock conversation: %w", err)
	}
	if archivedAt != nil {
		return ErrConversationArchived
	}
	if currentBytes+addedBytes > memoryConversationMaxBytes {
		return ErrConversationLimit
	}
	if title == "" {
		title = titleFromFirstUserMessage(pending)
	}
	if _, err := tx.Exec(ctx, `
		UPDATE chat_conversations
		SET messages = messages || $3::jsonb,
		    message_bytes = $4,
		    title = $5,
		    updated_at = $6
		WHERE id = $1 AND owner_sub = $2`, conversationID, ownerID, string(encoded), currentBytes+addedBytes, title, now); err != nil {
		return fmt.Errorf("append conversation messages: %w", err)
	}
	if err := tx.Commit(ctx); err != nil {
		return fmt.Errorf("commit append conversation: %w", err)
	}
	return nil
}

func scanPostgresConversation(row pgx.Row) (*Conversation, error) {
	var conversation Conversation
	var encoded []byte
	if err := row.Scan(
		&conversation.ID,
		&conversation.Title,
		&encoded,
		&conversation.CreatedAt,
		&conversation.UpdatedAt,
		&conversation.ArchivedAt,
	); err != nil {
		return nil, err
	}
	if err := json.Unmarshal(encoded, &conversation.Messages); err != nil {
		return nil, fmt.Errorf("decode conversation messages: %w", err)
	}
	return &conversation, nil
}
