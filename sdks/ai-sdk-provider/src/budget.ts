/**
 * In-memory session budget with reserve / finalize / release semantics.
 *
 * Prevents TOCTOU overspend when multiple concurrent logical requests race
 * against the same budget (§4.3 T1-A). JS is single-threaded: the
 * check-and-reserve step is a synchronous critical section — no mutex needed.
 *
 * State machine (§4.3):
 *   on 402 parsed with cost:
 *     if (available - sumReserved() < cost) throw SolvelaBudgetExceededError
 *     reserved.set(requestId, cost)
 *   on retry 2xx:
 *     available -= reserved.get(requestId); reserved.delete(requestId)
 *   on retry non-2xx / abort / signing error / network error:
 *     reserved.delete(requestId)  // release, no debit
 */

import { SolvelaBudgetExceededError } from './errors.js';

/**
 * Synchronous, in-memory budget tracker.
 * One instance per provider instance.
 *
 * If constructed with `total === undefined`, the budget is disabled and every
 * `reserve()` call succeeds with zero bookkeeping.
 */
export class BudgetState {
  /** `undefined` => budget disabled. */
  private available: bigint | undefined;
  /** Per-request reservations keyed by the per-invocation requestId. */
  private readonly reserved: Map<string, bigint> = new Map();

  constructor(total: bigint | undefined) {
    this.available = total;
  }

  /**
   * True when no budget cap was configured.
   */
  get isDisabled(): boolean {
    return this.available === undefined;
  }

  /**
   * Snapshot of the currently un-reserved, un-debited remainder.
   * Returns `undefined` when the budget is disabled.
   */
  get remaining(): bigint | undefined {
    if (this.available === undefined) return undefined;
    return this.available - this.sumReserved();
  }

  /**
   * Atomic check-and-reserve. Synchronous — no async boundary inside.
   *
   * @throws SolvelaBudgetExceededError if the reservation would drive the
   *         un-reserved remainder below zero.
   */
  reserve(requestId: string, cost: bigint): void {
    if (this.available === undefined) {
      // Budget disabled — all reservations succeed with no bookkeeping.
      return;
    }
    if (cost < 0n) {
      // Defensive: never reserve a negative amount.
      throw new SolvelaBudgetExceededError({
        message: `[solvela] budget reserve refused: cost ${cost.toString()} is negative`,
        url: '',
        requestBodyValues: undefined,
      });
    }
    const remaining = this.available - this.sumReserved();
    if (remaining < cost) {
      throw new SolvelaBudgetExceededError({
        message:
          `[solvela] session budget exceeded: request cost ${cost.toString()} exceeds ` +
          `remaining ${remaining.toString()} (total ${this.available.toString()}).`,
        url: '',
        requestBodyValues: undefined,
      });
    }
    this.reserved.set(requestId, cost);
  }

  /**
   * Finalize a reservation — debit from `available` and remove from the
   * reserved map. No-op if the reservation is absent (idempotent).
   */
  finalize(requestId: string): void {
    const cost = this.reserved.get(requestId);
    if (cost === undefined) return;
    this.reserved.delete(requestId);
    if (this.available !== undefined) {
      this.available -= cost;
    }
  }

  /**
   * Release a reservation without debiting. No-op if the reservation is
   * absent (idempotent).
   */
  release(requestId: string): void {
    this.reserved.delete(requestId);
  }

  private sumReserved(): bigint {
    let sum = 0n;
    for (const v of this.reserved.values()) {
      sum += v;
    }
    return sum;
  }
}
