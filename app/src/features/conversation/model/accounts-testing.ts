// The clock of the tests of the accounts and their limits (AC-Limits, AC-Out, the model
// sheet). Only test files import this module.

/** The ISO time of `hours`:`minutes` on 2026-11-`day` in the time zone of the test. */
export function at(hours: number, minutes: number, day = 14): string {
  return new Date(2026, 10, day, hours, minutes).toISOString();
}
