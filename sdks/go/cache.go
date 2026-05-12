package solvela

import (
	"crypto/sha256"
	"encoding/binary"
	"sync"
	"time"
)

type cacheEntry struct {
	response ChatResponse
	inserted time.Time
}

// ResponseCache is a thread-safe LRU cache for chat responses.
type ResponseCache struct {
	mu          sync.Mutex
	entries     map[uint64]*cacheEntry
	order       []uint64
	maxEntries  int
	ttl         time.Duration
	dedupWindow time.Duration
}

// NewResponseCache creates a cache with default settings (100 entries, 5m TTL, 2s dedup).
func NewResponseCache() *ResponseCache {
	return NewResponseCacheWithConfig(100, 5*time.Minute, 2*time.Second)
}

// NewResponseCacheWithConfig creates a cache with custom settings.
func NewResponseCacheWithConfig(maxEntries int, ttl, dedupWindow time.Duration) *ResponseCache {
	return &ResponseCache{
		entries:     make(map[uint64]*cacheEntry),
		order:       make([]uint64, 0),
		maxEntries:  maxEntries,
		ttl:         ttl,
		dedupWindow: dedupWindow,
	}
}

// CacheKey computes a deterministic key from model and messages.
func CacheKey(model string, messages []ChatMessage) uint64 {
	h := sha256.New()
	h.Write([]byte(model))
	for _, msg := range messages {
		h.Write([]byte(msg.Role))
		h.Write([]byte(msg.Content))
	}
	return binary.BigEndian.Uint64(h.Sum(nil)[:8])
}

// Get retrieves a cached response. Returns the response and true if found and not expired.
func (c *ResponseCache) Get(key uint64) (ChatResponse, bool) {
	c.mu.Lock()
	defer c.mu.Unlock()

	entry, ok := c.entries[key]
	if !ok {
		return ChatResponse{}, false
	}
	if time.Since(entry.inserted) > c.ttl {
		delete(c.entries, key)
		c.removeFromOrder(key)
		return ChatResponse{}, false
	}
	// Move to end of order (most recently used)
	c.removeFromOrder(key)
	c.order = append(c.order, key)
	return entry.response, true
}

// Put adds a response to the cache. If the cache is full, the least recently used entry is evicted.
// If the key was inserted within the dedup window, the put is ignored.
func (c *ResponseCache) Put(key uint64, response ChatResponse) {
	c.mu.Lock()
	defer c.mu.Unlock()

	// Check dedup window
	if existing, ok := c.entries[key]; ok {
		if time.Since(existing.inserted) < c.dedupWindow {
			return
		}
	}

	// Evict LRU if at capacity
	for len(c.entries) >= c.maxEntries && len(c.order) > 0 {
		oldest := c.order[0]
		c.order = c.order[1:]
		delete(c.entries, oldest)
	}

	c.entries[key] = &cacheEntry{
		response: response,
		inserted: time.Now(),
	}
	c.removeFromOrder(key)
	c.order = append(c.order, key)
}

func (c *ResponseCache) removeFromOrder(key uint64) {
	for i, k := range c.order {
		if k == key {
			c.order = append(c.order[:i], c.order[i+1:]...)
			return
		}
	}
}
