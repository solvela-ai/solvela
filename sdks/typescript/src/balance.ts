export class BalanceMonitor {
  private balance: number | undefined;
  private wasLow = false;
  private intervalId: ReturnType<typeof setInterval> | null = null;
  private stopped = false;

  constructor(
    private readonly fetchBalance: () => Promise<number>,
    private readonly pollInterval: number = 30,
    private readonly lowBalanceThreshold?: number,
    private readonly onLowBalance?: (balance: number) => void,
    private readonly onError?: (error: unknown) => void,
  ) {}

  start(): void {
    this.stopped = false;
    this.poll();
    this.intervalId = setInterval(() => this.poll(), this.pollInterval * 1000);
  }

  stop(): void {
    this.stopped = true;
    if (this.intervalId !== null) {
      clearInterval(this.intervalId);
      this.intervalId = null;
    }
  }

  lastKnownBalance(): number | undefined {
    return this.balance;
  }

  async pollOnce(): Promise<void> {
    return this.poll();
  }

  private async poll(): Promise<void> {
    if (this.stopped) return;
    try {
      const balance = await this.fetchBalance();
      this.balance = balance;
      if (this.lowBalanceThreshold !== undefined && this.onLowBalance) {
        const isLow = balance < this.lowBalanceThreshold;
        if (isLow && !this.wasLow) this.onLowBalance(balance);
        this.wasLow = isLow;
      }
    } catch (e) {
      // Surface poll errors. If the caller wired an onError handler we defer
      // to it; otherwise log a warning so a silent poll failure doesn't leave
      // `lastKnownBalance()` stuck at undefined and silently disable the
      // freeFallbackModel guard in SolvelaClient.
      if (this.onError) {
        this.onError(e);
      } else {
        console.warn('[BalanceMonitor] balance poll failed:', e);
      }
    }
  }
}
