// Pure helpers shared by the GUI shell. They hold the behaviours that are easy
// to regress and cheap to unit-test without a DOM: password whitespace rules,
// the progress reset applied to finished tasks, and the version fallback.

import type { Task, TaskStatus } from "./types";

/// Fields every finished task must reset so no stale progress text survives.
export const FINISHED_TASK_RESET: Pick<Task, "line" | "stage" | "stageDetail"> = {
  line: null,
  stage: undefined,
  stageDetail: null,
};

/// Only success/skipped are fully done; everything else has no percentage.
export function finishedPercent(status: TaskStatus | null): number | null {
  return status === "success" || status === "skipped" ? 100 : null;
}

/// Passwords may legitimately start or end with spaces: only "" is invalid,
/// matching upzip-core / upzip-cli.
export function isPasswordEmpty(raw: string): boolean {
  return raw.length === 0;
}

/// Wire form of the one-off password: `null` means "no password supplied".
export function passwordToSubmit(raw: string): string | null {
  return raw.length === 0 ? null : raw;
}

/// App version reported by the backend, falling back to the bundled one.
export function displayVersion(info: { version: string } | null, bundled: string): string {
  return info ? info.version : bundled;
}
