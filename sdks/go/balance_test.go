package solvela

import (
	"bytes"
	"errors"
	"log"
	"strings"
	"sync"
	"testing"
	"time"
)

func TestBalanceMonitorPollsAndUpdates(t *testing.T) {
	callCount := 0
	var mu sync.Mutex

	monitor := NewBalanceMonitor(
		func() (float64, error) {
			mu.Lock()
			defer mu.Unlock()
			callCount++
			return 100.0, nil
		},
		50*time.Millisecond,
		nil,
		nil,
	)

	monitor.Start()
	time.Sleep(80 * time.Millisecond)
	monitor.Stop()

	balance := monitor.LastKnownBalance()
	if balance == nil {
		t.Fatal("expected balance to be set")
	}
	if *balance != 100.0 {
		t.Errorf("balance: got %f, want 100.0", *balance)
	}

	mu.Lock()
	count := callCount
	mu.Unlock()
	if count < 1 {
		t.Errorf("expected at least 1 poll, got %d", count)
	}
}

func TestBalanceMonitorLowBalanceCallback(t *testing.T) {
	balance := 50.0
	threshold := 10.0
	var callbackBalance float64
	callbackCalled := false
	var mu sync.Mutex

	callIdx := 0

	monitor := NewBalanceMonitor(
		func() (float64, error) {
			mu.Lock()
			defer mu.Unlock()
			callIdx++
			if callIdx >= 3 {
				return 5.0, nil // Drop below threshold
			}
			return balance, nil
		},
		30*time.Millisecond,
		&threshold,
		func(b float64) {
			mu.Lock()
			defer mu.Unlock()
			callbackCalled = true
			callbackBalance = b
		},
	)

	monitor.Start()
	time.Sleep(150 * time.Millisecond)
	monitor.Stop()

	mu.Lock()
	defer mu.Unlock()
	if !callbackCalled {
		t.Error("expected low balance callback to be called")
	}
	if callbackBalance != 5.0 {
		t.Errorf("callback balance: got %f, want 5.0", callbackBalance)
	}
}

func TestBalanceMonitorTransitionDebounce(t *testing.T) {
	threshold := 10.0
	callbackCount := 0
	var mu sync.Mutex
	callIdx := 0

	monitor := NewBalanceMonitor(
		func() (float64, error) {
			mu.Lock()
			defer mu.Unlock()
			callIdx++
			// Stay below threshold for multiple polls
			return 5.0, nil
		},
		30*time.Millisecond,
		&threshold,
		func(b float64) {
			mu.Lock()
			defer mu.Unlock()
			callbackCount++
		},
	)

	monitor.Start()
	time.Sleep(150 * time.Millisecond)
	monitor.Stop()

	mu.Lock()
	defer mu.Unlock()
	// Should only fire once on transition, not every poll
	if callbackCount != 1 {
		t.Errorf("callback count: got %d, want 1 (should only fire on transition)", callbackCount)
	}
}

func TestBalanceMonitorStopIdempotent(t *testing.T) {
	monitor := NewBalanceMonitor(
		func() (float64, error) { return 100.0, nil },
		50*time.Millisecond,
		nil,
		nil,
	)

	monitor.Start()
	// Calling Stop multiple times should not panic
	monitor.Stop()
	monitor.Stop()
	monitor.Stop()
}

func TestBalanceMonitorNilBeforeStart(t *testing.T) {
	monitor := NewBalanceMonitor(
		func() (float64, error) { return 100.0, nil },
		50*time.Millisecond,
		nil,
		nil,
	)

	balance := monitor.LastKnownBalance()
	if balance != nil {
		t.Error("expected nil balance before start")
	}
}

func TestBalanceMonitorLogsAndRecoversAfterError(t *testing.T) {
	// Capture log output so we can assert the warning is emitted on the
	// failed first poll. Mirrors the warn-on-poll-error contract in the TS
	// SDK: a silent failure would leave LastKnownBalance() stuck at nil.
	var buf bytes.Buffer
	originalOutput := log.Writer()
	log.SetOutput(&buf)
	defer log.SetOutput(originalOutput)

	callIdx := 0
	var mu sync.Mutex

	monitor := NewBalanceMonitor(
		func() (float64, error) {
			mu.Lock()
			defer mu.Unlock()
			callIdx++
			if callIdx == 1 {
				return 0, errors.New("network error")
			}
			return 42.0, nil
		},
		30*time.Millisecond,
		nil,
		nil,
	)

	monitor.Start()
	time.Sleep(100 * time.Millisecond)
	monitor.Stop()

	balance := monitor.LastKnownBalance()
	if balance == nil {
		t.Fatal("expected balance after recovery")
	}
	if *balance != 42.0 {
		t.Errorf("balance: got %f, want 42.0", *balance)
	}

	logged := buf.String()
	if !strings.Contains(logged, "[BalanceMonitor]") {
		t.Errorf("expected log to contain '[BalanceMonitor]' tag; got: %q", logged)
	}
	if !strings.Contains(logged, "network error") {
		t.Errorf("expected log to contain underlying error message; got: %q", logged)
	}
}
