export type FileKind =
  | "archive"
  | "masked_archive"
  | "ssefe_encrypted"
  | "sfx_executable"
  | "not_archive"
  | "unknown";

export type PasswordState = "no_password" | "needs_password" | "unknown";

export type TaskStatus = "success" | "skipped" | "needs_password" | "failed" | "cancelled";

export type ConflictMode = "rename" | "skip" | "overwrite";
export type LayoutMode = "smart" | "folder" | "direct";
export type ThemeMode = "system" | "light" | "dark";

export interface Detection {
  path: string;
  size: number;
  kind: FileKind;
  container: string;
  format?: string | null;
  offset?: number | null;
  stub_size?: number | null;
  total_entries: number;
  encrypted_entries: number;
  top_level: string[];
  total_uncompressed: number;
  password_state: PasswordState;
  hints: string[];
  quick_entry?: string | null;
  listing_ok_no_pw: boolean;
  office_like: boolean;
  carved_path?: string | null;
  carved_from?: number | null;
  fingerprint?: string | null;
  notes: string[];
}

export interface TaskOutcome {
  task_id: number;
  status: TaskStatus | null;
  path: string;
  detection: Detection | null;
  dest?: string | null;
  files: number;
  bytes: number;
  reason?: string | null;
  password_source?: string | null;
  passwords_tried: number;
  elapsed_ms: number;
  warnings?: string[];
  related_paths?: string[];
}

export interface Summary {
  total: number;
  success: number;
  skipped: number;
  needs_password: number;
  failed: number;
  cancelled: number;
  bytes: number;
  elapsed_ms: number;
}

export type TaskEvent =
  | { type: "task_queued"; id: number; path: string; size: number }
  | { type: "task_stage"; id: number; stage: string; detail?: string | null }
  | { type: "task_detected"; id: number; detection: Detection }
  | { type: "task_progress"; id: number; percent: number; line?: string | null }
  | { type: "task_password_attempt"; id: number; source: string; ok: boolean; tried: number }
  | { type: "task_password_missing"; id: number; tried: number }
  | { type: "task_finished"; id: number; outcome: TaskOutcome }
  | { type: "log"; line: string };

export interface Settings {
  outputRoot: string | null;
  jobs: number;
  conflict: ConflictMode;
  layout: LayoutMode;
  skipProcessed: boolean;
  forceDocuments: boolean;
  keepBroken: boolean;
  stripJunk: boolean;
  nested: boolean;
  maxDepth: number;
  stopOnError: boolean;
  tryEmptyPassword: boolean;
  allowExecuteSfx: boolean;
  recursive: boolean;
  minFreeRatio: number;
  theme: ThemeMode;
  settingsVersion?: number;
}

export interface AppInfo {
  version: string;
  configDir: string;
  configPath: string;
  settingsPath: string;
  sevenZip: string | null;
  sevenZipVersion: string | null;
  logPath?: string | null;
  logDir?: string | null;
  stagingPrefix: string;
}

export interface Diagnostics {
  version?: string;
  os?: string;
  arch?: string;
  configDir?: string;
  settingsPath?: string;
  sevenZip?: string | null;
  sevenZipVersion?: string | null;
  logPath?: string | null;
  logDir?: string | null;
  processedPath?: string | null;
  processedCount?: number;
  passwordCount?: number;
  stagingPrefix?: string;
  settingsJson?: string;
}

export interface BatchState {
  running: boolean;
  paused: boolean;
  summary: Summary | null;
}

export interface StartRequest {
  files: string[];
  password: string | null;
  rememberPassword: boolean;
  settings: Settings;
}

export interface Task {
  key: string;
  id?: number | null;
  path: string;
  name: string;
  size?: number | null;
  phase: "queued" | "running" | "done";
  status?: TaskStatus | null;
  stage?: string;
  stageDetail?: string | null;
  percent?: number | null;
  line?: string | null;
  dest?: string | null;
  reason?: string | null;
  warnings?: string[];
  warningsOpen?: boolean;
  files?: number;
  bytes?: number;
  elapsed_ms?: number;
  password_source?: string | null;
  detection?: Detection | null;
}

export type LiveCounts = Omit<Summary, "elapsed_ms">;

export type Tab = "queue" | "passwords" | "logs" | "settings";

export const DEFAULT_SETTINGS: Settings = {
  outputRoot: null,
  jobs: 2,
  conflict: "rename",
  layout: "smart",
  skipProcessed: false,
  forceDocuments: false,
  keepBroken: false,
  stripJunk: true,
  nested: true,
  maxDepth: 4,
  stopOnError: false,
  tryEmptyPassword: true,
  allowExecuteSfx: false,
  recursive: true,
  minFreeRatio: 1.1,
  theme: "system",
  settingsVersion: 2,
};

export const REQUEUE: Partial<Task> = {
  phase: "queued",
  status: null,
  stage: undefined,
  stageDetail: null,
  percent: null,
  line: null,
  dest: null,
  reason: null,
  warnings: undefined,
  warningsOpen: false,
  files: undefined,
  bytes: undefined,
  elapsed_ms: undefined,
  password_source: null,
};

export const EMPTY_COUNTS: LiveCounts = {
  total: 0,
  success: 0,
  skipped: 0,
  needs_password: 0,
  failed: 0,
  cancelled: 0,
  bytes: 0,
};
