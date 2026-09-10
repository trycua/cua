CREATE TABLE public.chat_conversations (
    id text PRIMARY KEY,
    owner_sub text NOT NULL,
    title text NOT NULL DEFAULT '',
    messages jsonb NOT NULL DEFAULT '[]'::jsonb,
    message_bytes bigint NOT NULL DEFAULT 0 CHECK (message_bytes >= 0),
    created_at timestamptz NOT NULL,
    updated_at timestamptz NOT NULL,
    archived_at timestamptz,
    CONSTRAINT chat_conversations_messages_array CHECK (jsonb_typeof(messages) = 'array')
);
CREATE INDEX chat_conversations_owner_active_idx
    ON public.chat_conversations (owner_sub, updated_at DESC, id)
    WHERE archived_at IS NULL;
CREATE INDEX chat_conversations_owner_archived_idx
    ON public.chat_conversations (owner_sub, archived_at DESC, id)
    WHERE archived_at IS NOT NULL;
REVOKE ALL ON TABLE public.chat_conversations FROM PUBLIC;
GRANT SELECT, INSERT, UPDATE, DELETE ON TABLE public.chat_conversations TO cyclops_app;
