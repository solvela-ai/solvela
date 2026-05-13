import { describe, it, expect, vi } from 'vitest';
import { BalanceMonitor } from '../../src/balance.js';

describe('BalanceMonitor', () => {
  it('lastKnownBalance is undefined before polling', () => {
    const monitor = new BalanceMonitor(async () => 1000);
    expect(monitor.lastKnownBalance()).toBeUndefined();
  });

  it('pollOnce updates balance', async () => {
    const fetchBalance = vi.fn().mockResolvedValue(5000);
    const monitor = new BalanceMonitor(fetchBalance, 30);
    await monitor.pollOnce();
    expect(monitor.lastKnownBalance()).toBe(5000);
    expect(fetchBalance).toHaveBeenCalledTimes(1);
  });

  it('pollOnce calls onLowBalance on transition to low', async () => {
    let currentBalance = 5000;
    const fetchBalance = vi.fn().mockImplementation(async () => currentBalance);
    const onLow = vi.fn();
    const monitor = new BalanceMonitor(fetchBalance, 30, 1000, onLow);

    // First poll - balance is fine
    await monitor.pollOnce();
    expect(onLow).not.toHaveBeenCalled();

    // Drop balance below threshold
    currentBalance = 500;
    await monitor.pollOnce();
    expect(onLow).toHaveBeenCalledTimes(1);
    expect(onLow).toHaveBeenCalledWith(500);
  });

  it('does not call onLowBalance repeatedly while still low', async () => {
    let currentBalance = 500;
    const fetchBalance = vi.fn().mockImplementation(async () => currentBalance);
    const onLow = vi.fn();
    const monitor = new BalanceMonitor(fetchBalance, 30, 1000, onLow);

    // First poll - triggers low balance
    await monitor.pollOnce();
    expect(onLow).toHaveBeenCalledTimes(1);

    // Second poll - still low, should NOT trigger again
    currentBalance = 400;
    await monitor.pollOnce();
    expect(onLow).toHaveBeenCalledTimes(1);
  });

  it('calls onLowBalance again after recovery and drop', async () => {
    let currentBalance = 500;
    const fetchBalance = vi.fn().mockImplementation(async () => currentBalance);
    const onLow = vi.fn();
    const monitor = new BalanceMonitor(fetchBalance, 30, 1000, onLow);

    // Initial: low
    await monitor.pollOnce();
    expect(onLow).toHaveBeenCalledTimes(1);

    // Recover
    currentBalance = 5000;
    await monitor.pollOnce();

    // Drop again
    currentBalance = 200;
    await monitor.pollOnce();
    expect(onLow).toHaveBeenCalledTimes(2);
  });

  it('stop is idempotent', () => {
    const monitor = new BalanceMonitor(async () => 1000);
    monitor.start();
    monitor.stop();
    monitor.stop(); // should not throw
  });

  it('logs a warning on fetch error when no onError handler is wired', async () => {
    const warnSpy = vi.spyOn(console, 'warn').mockImplementation(() => {});
    const fetchBalance = vi.fn().mockRejectedValue(new Error('network'));
    const monitor = new BalanceMonitor(fetchBalance, 30);

    // Should not throw
    await monitor.pollOnce();
    expect(monitor.lastKnownBalance()).toBeUndefined();
    expect(warnSpy).toHaveBeenCalledTimes(1);
    expect(warnSpy.mock.calls[0][0]).toContain('[BalanceMonitor]');
    warnSpy.mockRestore();
  });

  it('routes fetch errors to onError handler when wired (no warn)', async () => {
    const warnSpy = vi.spyOn(console, 'warn').mockImplementation(() => {});
    const fetchBalance = vi.fn().mockRejectedValue(new Error('network'));
    const onError = vi.fn();
    const monitor = new BalanceMonitor(fetchBalance, 30, undefined, undefined, onError);

    await monitor.pollOnce();
    expect(onError).toHaveBeenCalledTimes(1);
    expect(onError.mock.calls[0][0]).toBeInstanceOf(Error);
    expect(warnSpy).not.toHaveBeenCalled();
    warnSpy.mockRestore();
  });

  it('start and stop manage interval', () => {
    vi.useFakeTimers();
    const fetchBalance = vi.fn().mockResolvedValue(1000);
    const monitor = new BalanceMonitor(fetchBalance, 10);
    monitor.start();
    // Should have called poll once immediately
    expect(fetchBalance).toHaveBeenCalledTimes(1);
    monitor.stop();
    vi.useRealTimers();
  });
});
