package solvela

import (
	"log"
	"sync"
	"time"
)

// BalanceMonitor periodically checks wallet balance and fires callbacks on low balance.
type BalanceMonitor struct {
	fetchBalance        func() (float64, error)
	pollInterval        time.Duration
	lowBalanceThreshold *float64
	onLowBalance        func(float64)
	onPoll              func(float64)

	mu      sync.Mutex
	balance *float64
	wasLow  bool
	stopCh  chan struct{}
	stopped bool
}

// NewBalanceMonitor creates a new balance monitor.
func NewBalanceMonitor(
	fetchBalance func() (float64, error),
	pollInterval time.Duration,
	lowBalanceThreshold *float64,
	onLowBalance func(float64),
) *BalanceMonitor {
	return &BalanceMonitor{
		fetchBalance:        fetchBalance,
		pollInterval:        pollInterval,
		lowBalanceThreshold: lowBalanceThreshold,
		onLowBalance:        onLowBalance,
		stopCh:              make(chan struct{}),
	}
}

// SetOnPoll registers a callback that fires after each successful balance
// poll. The callback runs on the monitor's polling goroutine, so it must
// not block. Call SetOnPoll before [BalanceMonitor.Start]; after Start the
// callback is read without locking.
func (m *BalanceMonitor) SetOnPoll(cb func(float64)) {
	m.onPoll = cb
}

// Start begins polling in a background goroutine.
func (m *BalanceMonitor) Start() {
	go m.run()
}

// Stop halts the polling loop. Safe to call multiple times.
func (m *BalanceMonitor) Stop() {
	m.mu.Lock()
	defer m.mu.Unlock()
	if !m.stopped {
		m.stopped = true
		close(m.stopCh)
	}
}

// LastKnownBalance returns the most recently fetched balance, or nil if none.
func (m *BalanceMonitor) LastKnownBalance() *float64 {
	m.mu.Lock()
	defer m.mu.Unlock()
	return m.balance
}

func (m *BalanceMonitor) run() {
	ticker := time.NewTicker(m.pollInterval)
	defer ticker.Stop()

	m.poll() // initial poll

	for {
		select {
		case <-m.stopCh:
			return
		case <-ticker.C:
			m.poll()
		}
	}
}

func (m *BalanceMonitor) poll() {
	balance, err := m.fetchBalance()
	if err != nil {
		// Surface poll errors. A silent failure would leave LastKnownBalance
		// stuck at nil and silently disable the freeFallbackModel guard in
		// SolvelaClient. Mirrors the warn-on-poll-error fix in the TS SDK.
		log.Printf("[BalanceMonitor] balance poll failed: %v", err)
		return
	}

	m.mu.Lock()
	m.balance = &balance
	onPoll := m.onPoll
	onLow := m.onLowBalance
	threshold := m.lowBalanceThreshold
	wasLow := m.wasLow
	if threshold != nil && onLow != nil {
		m.wasLow = balance < *threshold
	}
	m.mu.Unlock()

	if onPoll != nil {
		onPoll(balance)
	}
	if threshold != nil && onLow != nil {
		isLow := balance < *threshold
		if isLow && !wasLow {
			onLow(balance)
		}
	}
}
