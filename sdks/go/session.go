package solvela

import (
	"crypto/rand"
	"crypto/sha256"
	"encoding/binary"
	"fmt"
	"sync"
	"time"
)

// SessionInfo holds the current state of a session.
type SessionInfo struct {
	Model     string
	Escalated bool
}

type sessionEntry struct {
	model        string
	created      time.Time
	recentHashes []uint64
	escalated    bool
}

// SessionStore manages conversation sessions with model escalation.
type SessionStore struct {
	mu       sync.Mutex
	sessions map[string]*sessionEntry
	ttl      time.Duration
}

// NewSessionStore creates a session store with the given TTL.
func NewSessionStore(ttl time.Duration) *SessionStore {
	return &SessionStore{
		sessions: make(map[string]*sessionEntry),
		ttl:      ttl,
	}
}

// GetOrCreate returns session info for the given ID, creating a new session if needed.
// Expired sessions are replaced with fresh ones.
//
// GetOrCreate is read-only with respect to escalation: it never advances the
// three-strike counter. That is the exclusive job of
// [SessionStore.RecordRequest], invoked once per successful request from the
// client.
func (s *SessionStore) GetOrCreate(sessionID, defaultModel string) SessionInfo {
	s.mu.Lock()
	defer s.mu.Unlock()

	entry, ok := s.sessions[sessionID]
	if !ok || time.Since(entry.created) > s.ttl {
		s.sessions[sessionID] = &sessionEntry{
			model:        defaultModel,
			created:      time.Now(),
			recentHashes: make([]uint64, 0),
			escalated:    false,
		}
		return SessionInfo{Model: defaultModel, Escalated: false}
	}

	return SessionInfo{Model: entry.model, Escalated: entry.escalated}
}

// RecordRequest records a request hash. If 3 or more identical hashes appear,
// the session is escalated (three-strike rule).
func (s *SessionStore) RecordRequest(sessionID string, requestHash uint64) {
	s.mu.Lock()
	defer s.mu.Unlock()

	entry, ok := s.sessions[sessionID]
	if !ok {
		return
	}

	entry.recentHashes = append(entry.recentHashes, requestHash)

	// Count occurrences of this hash
	count := 0
	for _, h := range entry.recentHashes {
		if h == requestHash {
			count++
		}
	}
	if count >= 3 {
		entry.escalated = true
	}
}

// CleanupExpired removes all sessions that have exceeded their TTL.
func (s *SessionStore) CleanupExpired() {
	s.mu.Lock()
	defer s.mu.Unlock()

	for id, entry := range s.sessions {
		if time.Since(entry.created) > s.ttl {
			delete(s.sessions, id)
		}
	}
}

// DeriveSessionID generates a deterministic session ID from the first message.
//
// Deprecated: this function keys only on the first message, which means
// distinct conversations that happen to share the same opening message
// collide on the same session. Prefer [DeriveSessionIDWithSalt], which mixes
// in a per-client random salt and the message count to avoid that collision.
func DeriveSessionID(messages []ChatMessage) string {
	h := sha256.New()
	if len(messages) > 0 {
		h.Write([]byte(messages[0].Role))
		h.Write([]byte(messages[0].Content))
	}
	return fmt.Sprintf("%x", h.Sum(nil)[:16])
}

// DeriveSessionIDWithSalt generates a session ID that mixes a per-client salt,
// the message count, and the first message. The salt prevents cross-client
// session aliasing, and the message count separates conversations that begin
// with the same prompt but evolve differently.
func DeriveSessionIDWithSalt(salt []byte, messages []ChatMessage) string {
	h := sha256.New()
	h.Write(salt)
	var countBuf [8]byte
	binary.BigEndian.PutUint64(countBuf[:], uint64(len(messages)))
	h.Write(countBuf[:])
	if len(messages) > 0 {
		h.Write([]byte(messages[0].Role))
		h.Write([]byte(messages[0].Content))
	}
	return fmt.Sprintf("%x", h.Sum(nil)[:16])
}

// newSessionSalt returns 8 bytes of cryptographically random salt for use as
// a per-client session-ID salt. Returns an error only if the system entropy
// source is unavailable.
func newSessionSalt() ([]byte, error) {
	salt := make([]byte, 8)
	if _, err := rand.Read(salt); err != nil {
		return nil, err
	}
	return salt, nil
}
