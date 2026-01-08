# Chat Threads for CUA Docs Assistant

## Overview

Add client-side conversation threads to the CopilotKit-powered docs assistant, allowing users to have multiple conversations with fresh context and return to previous sessions.

## Requirements

- Multiple conversations stored in browser localStorage (persists across tab/browser close)
- Toggle between thread list view and chat view (like messaging apps)
- AI-generated titles after first exchange
- No server-side storage changes (keep in-memory backend)
- Use CopilotKit's native `threadId`/`setThreadId` for thread switching

## Data Model

```ts
interface Thread {
  id: string;           // UUID
  title: string;        // AI-generated, defaults to "New conversation"
  messages: Message[];  // CopilotKit message format
  createdAt: number;    // timestamp
  updatedAt: number;    // timestamp
}

interface ThreadsState {
  threads: Thread[];
  activeThreadId: string | null;
  view: 'list' | 'chat';
}
```

## UI Design

### Thread List View
- Header: "Conversations" title + "New Chat" button (+ icon)
- List items: AI-generated title + timestamp + first message preview (truncated, faded)
- Tap thread → switch to Chat View with that thread loaded
- Empty state: "No conversations yet" message

### Chat View
- Header: Back arrow (←) + thread title + existing debug dropdown
- Back arrow → returns to Thread List View
- Chat area: unchanged (CopilotKit handles rendering)

### View Switching
- State-driven via `view: 'list' | 'chat'`
- New Chat → creates thread with UUID, switches to chat view
- Select existing thread → loads messages, switches to chat view
- Back arrow → shows list view (state persists)

## Title Generation

- **Trigger:** After first assistant response completes
- **Method:** Reuse CopilotKit runtime via `useCopilotAction`
- **Prompt:** Generate 3-6 word title from first user message + assistant response
- **Fallback:** "New conversation" until title is generated

## Architecture

### New Files

1. **`src/components/chat/ThreadsProvider.tsx`**
   - React context for threads state management
   - localStorage sync on every state change
   - Thread CRUD: create, update, delete, switch active

2. **`src/components/chat/ThreadListView.tsx`**
   - Renders list of threads with title, timestamp, preview
   - "New Chat" button
   - Thread selection handler

3. **`src/components/chat/ChatHeader.tsx`**
   - Custom CopilotKit header component
   - Back button + thread title display
   - Integrates with existing debug dropdown

4. **`src/components/chat/DocsAssistantChat.tsx`**
   - Main component orchestrating view switching
   - Renders ThreadListView or CopilotPopup based on view state
   - Replaces direct CopilotPopup usage in provider

### Modified Files

1. **`src/providers/copilotkit-provider.tsx`**
   - Wrap with ThreadsProvider
   - Pass dynamic `threadId` from threads state to CopilotKit
   - Register title generation action via `useCopilotAction`

### Data Flow

```
ThreadsProvider (state + localStorage sync)
  └── DocsAssistantChat (view switching logic)
        ├── ThreadListView (when view='list')
        └── CopilotPopup + ChatHeader (when view='chat')
              └── CopilotKit context (threadId from ThreadsProvider)
```

## Storage

- **Mechanism:** localStorage
- **Key:** `cua-docs-assistant-threads`
- **Limits:** No auto-cleanup (5MB limit unlikely to be hit for text conversations)
- **Persistence:** Survives tab close, browser restart; cleared on storage clear

## Implementation Notes

- Use CopilotKit's `useCopilotContext()` for `setThreadId`
- Custom Header component via CopilotPopup's `Header` prop
- Generate UUIDs with `crypto.randomUUID()`
- Title generation uses existing Anthropic runtime, no new endpoints
