import assert from "node:assert/strict";
import test from "node:test";

import {
  FINISHED_TASK_RESET,
  displayVersion,
  finishedPercent,
  isPasswordEmpty,
  passwordToSubmit,
} from "../src/task-logic.ts";
import { DEFAULT_SETTINGS } from "../src/types.ts";

test("passwords keep leading and trailing spaces", () => {
  assert.equal(isPasswordEmpty(""), true);
  assert.equal(isPasswordEmpty(" "), false);
  assert.equal(isPasswordEmpty("  secret  "), false);
  assert.equal(passwordToSubmit("  secret  "), "  secret  ");
  assert.equal(passwordToSubmit(""), null);
});

test("finished tasks always clear stale progress text", () => {
  assert.ok("line" in FINISHED_TASK_RESET && FINISHED_TASK_RESET.line === null);
  assert.ok("stage" in FINISHED_TASK_RESET && FINISHED_TASK_RESET.stage === undefined);
  assert.ok("stageDetail" in FINISHED_TASK_RESET && FINISHED_TASK_RESET.stageDetail === null);
});

test("percent is only set for success and skipped", () => {
  assert.equal(finishedPercent("success"), 100);
  assert.equal(finishedPercent("skipped"), 100);
  assert.equal(finishedPercent(null), null);
  for (const status of ["failed", "cancelled", "needs_password"] as const) {
    assert.equal(finishedPercent(status), null);
  }
});

test("version falls back to the bundled package version", () => {
  assert.equal(displayVersion({ version: "9.9.9" }, "0.4.0"), "9.9.9");
  assert.equal(displayVersion(null, "0.4.0"), "0.4.0");
});

test("GUI defaults match upzip-core", () => {
  assert.equal(DEFAULT_SETTINGS.tryEmptyPassword, false);
});
