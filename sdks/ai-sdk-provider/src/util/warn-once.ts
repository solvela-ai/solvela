/**
 * Memoized console.warn that emits each unique message at most once per
 * process lifetime. Multiple call sites sharing the same message string
 * produce exactly one log line.
 */

const emitted = new Set<string>();

/**
 * Emit a console.warn for `message` the first time this function is called
 * with that message in the current process. Subsequent calls with the same
 * message are no-ops.
 *
 * @param message - The warning message to emit (at most once).
 */
export function warnOnce(message: string): void {
  if (!emitted.has(message)) {
    emitted.add(message);
    console.warn(message);
  }
}
