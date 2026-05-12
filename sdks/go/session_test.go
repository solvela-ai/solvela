package solvela

import (
	"testing"
	"time"
)

func TestNewSession(t *testing.T) {
	store := NewSessionStore(30 * time.Minute)
	info := store.GetOrCreate("sess-1", "gpt-4")

	if info.Model != "gpt-4" {
		t.Errorf("model: got %q, want %q", info.Model, "gpt-4")
	}
	if info.Escalated {
		t.Error("new session should not be escalated")
	}
}

func TestExistingSession(t *testing.T) {
	store := NewSessionStore(30 * time.Minute)

	// Create session
	store.GetOrCreate("sess-1", "gpt-4")

	// Access again — should return same model
	info := store.GetOrCreate("sess-1", "gpt-3.5-turbo")
	if info.Model != "gpt-4" {
		t.Errorf("model: got %q, want %q (should keep original model)", info.Model, "gpt-4")
	}
}

func TestExpiredSession(t *testing.T) {
	store := NewSessionStore(50 * time.Millisecond)

	store.GetOrCreate("sess-1", "gpt-4")

	time.Sleep(60 * time.Millisecond)

	// Expired session should be replaced
	info := store.GetOrCreate("sess-1", "gpt-3.5-turbo")
	if info.Model != "gpt-3.5-turbo" {
		t.Errorf("model: got %q, want %q (expired session should use new default)", info.Model, "gpt-3.5-turbo")
	}
}

func TestThreeStrikeEscalation(t *testing.T) {
	store := NewSessionStore(30 * time.Minute)
	store.GetOrCreate("sess-1", "gpt-4")

	hash := uint64(12345)

	// Record same hash 3 times
	store.RecordRequest("sess-1", hash)
	info := store.GetOrCreate("sess-1", "gpt-4")
	if info.Escalated {
		t.Error("should not escalate after 1 strike")
	}

	store.RecordRequest("sess-1", hash)
	info = store.GetOrCreate("sess-1", "gpt-4")
	if info.Escalated {
		t.Error("should not escalate after 2 strikes")
	}

	store.RecordRequest("sess-1", hash)
	info = store.GetOrCreate("sess-1", "gpt-4")
	if !info.Escalated {
		t.Error("should escalate after 3 strikes")
	}
}

func TestThreeStrikeDifferentHashes(t *testing.T) {
	store := NewSessionStore(30 * time.Minute)
	store.GetOrCreate("sess-1", "gpt-4")

	// Different hashes should not trigger escalation
	store.RecordRequest("sess-1", 1)
	store.RecordRequest("sess-1", 2)
	store.RecordRequest("sess-1", 3)

	info := store.GetOrCreate("sess-1", "gpt-4")
	if info.Escalated {
		t.Error("different hashes should not trigger escalation")
	}
}

func TestCleanupExpired(t *testing.T) {
	store := NewSessionStore(50 * time.Millisecond)

	store.GetOrCreate("sess-1", "gpt-4")
	store.GetOrCreate("sess-2", "gpt-4")

	time.Sleep(60 * time.Millisecond)

	// Create a fresh session that should survive cleanup
	store.GetOrCreate("sess-3", "gpt-4")

	store.CleanupExpired()

	// sess-1 and sess-2 should be gone
	info1 := store.GetOrCreate("sess-1", "new-model")
	if info1.Model != "new-model" {
		t.Error("sess-1 should have been cleaned up and recreated")
	}

	info2 := store.GetOrCreate("sess-2", "new-model")
	if info2.Model != "new-model" {
		t.Error("sess-2 should have been cleaned up and recreated")
	}

	// sess-3 should still exist
	info3 := store.GetOrCreate("sess-3", "different-model")
	if info3.Model != "gpt-4" {
		t.Error("sess-3 should still exist with original model")
	}
}

func TestDeriveSessionID(t *testing.T) {
	msgs := []ChatMessage{
		{Role: RoleSystem, Content: "You are a helpful assistant."},
		{Role: RoleUser, Content: "Hello"},
	}

	id1 := DeriveSessionID(msgs)
	id2 := DeriveSessionID(msgs)
	if id1 != id2 {
		t.Errorf("same messages should produce same session ID: %q != %q", id1, id2)
	}

	if len(id1) != 32 {
		t.Errorf("session ID should be 32 hex chars: got %d", len(id1))
	}

	// Different first message should produce different ID
	msgs2 := []ChatMessage{
		{Role: RoleSystem, Content: "You are a different assistant."},
		{Role: RoleUser, Content: "Hello"},
	}
	id3 := DeriveSessionID(msgs2)
	if id1 == id3 {
		t.Error("different first messages should produce different session IDs")
	}
}

func TestDeriveSessionIDEmpty(t *testing.T) {
	id := DeriveSessionID([]ChatMessage{})
	if len(id) != 32 {
		t.Errorf("empty messages should still produce valid session ID: got len %d", len(id))
	}
}

func TestRecordRequestNonexistentSession(t *testing.T) {
	store := NewSessionStore(30 * time.Minute)
	// Should not panic
	store.RecordRequest("nonexistent", 12345)
}

// TestGetOrCreateDoesNotIncrementRequestCount asserts that calling
// GetOrCreate multiple times does not, by itself, advance the three-strike
// escalation counter. Counting is RecordRequest's job; previously
// GetOrCreate double-counted and could trigger escalation prematurely.
func TestGetOrCreateDoesNotIncrementRequestCount(t *testing.T) {
	store := NewSessionStore(30 * time.Minute)
	store.GetOrCreate("sess-1", "gpt-4")

	// Five GetOrCreate calls with the same id should not escalate, because
	// nothing has been RecordRequest'd yet.
	for i := 0; i < 5; i++ {
		info := store.GetOrCreate("sess-1", "gpt-4")
		if info.Escalated {
			t.Fatalf("GetOrCreate alone must not escalate (iteration %d)", i)
		}
	}

	// Two RecordRequests with the same hash plus more GetOrCreates still
	// must not escalate — only three RecordRequests do.
	store.RecordRequest("sess-1", 7)
	store.RecordRequest("sess-1", 7)
	for i := 0; i < 3; i++ {
		info := store.GetOrCreate("sess-1", "gpt-4")
		if info.Escalated {
			t.Fatalf("escalation triggered by GetOrCreate after only 2 RecordRequests (iter %d)", i)
		}
	}

	// Third RecordRequest crosses the threshold.
	store.RecordRequest("sess-1", 7)
	info := store.GetOrCreate("sess-1", "gpt-4")
	if !info.Escalated {
		t.Fatal("expected escalation after 3 RecordRequests with same hash")
	}
}

