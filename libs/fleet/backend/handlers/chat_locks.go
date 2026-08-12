package handlers

import "sync"

type conversationLock struct {
	mu   sync.Mutex
	refs int
}

type conversationLockRegistry struct {
	mu    sync.Mutex
	locks map[string]*conversationLock
}

func newConversationLockRegistry() *conversationLockRegistry {
	return &conversationLockRegistry{locks: make(map[string]*conversationLock)}
}

func (registry *conversationLockRegistry) lock(conversationID string) func() {
	registry.mu.Lock()
	entry := registry.locks[conversationID]
	if entry == nil {
		entry = &conversationLock{}
		registry.locks[conversationID] = entry
	}
	entry.refs++
	registry.mu.Unlock()

	entry.mu.Lock()
	return func() {
		entry.mu.Unlock()
		registry.mu.Lock()
		entry.refs--
		if entry.refs == 0 && registry.locks[conversationID] == entry {
			delete(registry.locks, conversationID)
		}
		registry.mu.Unlock()
	}
}

var fallbackConversationLocks = newConversationLockRegistry()

func (h Handlers) lockConversation(conversationID string) func() {
	if h.chatLocks == nil {
		return fallbackConversationLocks.lock(conversationID)
	}
	return h.chatLocks.lock(conversationID)
}
