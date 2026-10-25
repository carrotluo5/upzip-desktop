import { Fragment, useCallback, useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { confirm as confirmDialog, open as openDialog } from "@tauri-apps/plugin-dialog";
import "./App.css";

/* ================= 契约类型（docs/DEV-CONTRACT.md §2 / §4） ================= */

type FileKind =
  | "archive"
  | "masked_archive"
  | "ssefe_encrypted"
  | "sfx_executable"
  | "not_archive"
  | "unknown";

type PasswordState = "no_password" | "needs_password" | "unknown";

type TaskStatus = "success" | "skipped" | "needs_password" | "failed" | "cancelled";

type ConflictMode = "rename" | "skip" | "overwrite";
type LayoutMode = "smart" | "folder" | "direct";
type ThemeMode = "system" | "light" | "dark";

/* Detection 保持 engine types.rs 的 serde 形状（snake_case）。 */
interface Detection {
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

interface TaskOutcome {
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

interface Summary {
  total: number;
  success: number;
  skipped: number;
  needs_password: number;
  failed: number;
  cancelled: number;
  bytes: number;
  elapsed_ms: number;
}

type TaskEvent =
  | { type: "task_queued"; id: number; path: string; size: number }
  | { type: "task_stage"; id: number; stage: string; detail?: string | null }
  | { type: "task_detected"; id: number; detection: Detection }
  | { type: "task_progress"; id: number; percent: number; line?: string | null }
  | { type: "task_password_attempt"; id: number; source: string; ok: boolean; tried: number }
  | { type: "task_password_missing"; id: number; tried: number }
  | { type: "task_finished"; id: number; outcome: TaskOutcome }
  | { type: "log"; line: string };

interface Settings {
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

interface AppInfo {
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

interface Diagnostics {
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

interface BatchState {
  running: boolean;
  paused: boolean;
  summary: Summary | null;
}

interface StartRequest {
  files: string[];
  password: string | null;
  rememberPassword: boolean;
  settings: Settings;
}

/* ================= 常量 / 工具函数 ================= */

const DEFAULT_SETTINGS: Settings = {
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

const BACKSLASH = String.fromCharCode(92);

function basename(p: string): string {
  const idx = Math.max(p.lastIndexOf("/"), p.lastIndexOf(BACKSLASH));
  return idx >= 0 ? p.slice(idx + 1) : p;
}

function humanBytes(n: number): string {
  if (!Number.isFinite(n) || n <= 0) return "0 B";
  if (n < 1024) return n + " B";
  const units = ["KB", "MB", "GB", "TB", "PB"];
  let value = n / 1024;
  let i = 0;
  while (value >= 1024 && i < units.length - 1) {
    value /= 1024;
    i += 1;
  }
  return (value >= 100 ? value.toFixed(0) : value.toFixed(1)) + " " + units[i];
}

function kindLabel(kind: FileKind): string {
  switch (kind) {
    case "archive":
      return "压缩包";
    case "masked_archive":
      return "伪装压缩包";
    case "ssefe_encrypted":
      return ".enc 加密容器";
    case "sfx_executable":
      return "自解压 EXE";
    case "not_archive":
      return "非压缩文件";
    case "unknown":
      return "未知";
    default:
      return "未知";
  }
}

function passwordStateLabel(state: PasswordState): string {
  switch (state) {
    case "no_password":
      return "无密码";
    case "needs_password":
      return "需要密码";
    case "unknown":
      return "未知";
    default:
      return "未知";
  }
}

function statusLabel(status: TaskStatus | null | undefined): string {
  switch (status) {
    case "success":
      return "成功";
    case "skipped":
      return "已跳过";
    case "needs_password":
      return "需密码";
    case "failed":
      return "失败";
    case "cancelled":
      return "已取消";
    default:
      return "等待";
  }
}

function stageLabel(stage: string | null | undefined): string {
  switch (stage) {
    case "detect":
      return "识别";
    case "decrypt":
      return "解密 SSEFE";
    case "extract":
      return "解压";
    case "nested":
      return "递归内层";
    case "land":
      return "写出文件";
    case "password":
      return "尝试密码";
    case "cleanup":
      return "清理";
    case "done":
      return "完成";
    case null:
    case undefined:
    case "":
      return "处理中";
    default:
      return stage;
  }
}

function errText(error: unknown): string {
  if (error instanceof Error) return error.message;
  if (typeof error === "string") return error;
  try {
    return JSON.stringify(error);
  } catch {
    return String(error);
  }
}

function toPath(value: unknown): string | null {
  if (typeof value === "string") {
    const trimmed = value.trim();
    return trimmed.length > 0 ? trimmed : null;
  }
  if (value && typeof value === "object" && "path" in value) {
    const path = (value as { path?: unknown }).path;
    if (typeof path === "string") {
      const trimmed = path.trim();
      return trimmed.length > 0 ? trimmed : null;
    }
  }
  return null;
}

function asPathList(value: unknown): string[] {
  if (!Array.isArray(value)) return [];
  const out: string[] = [];
  for (const item of value) {
    const path = toPath(item);
    if (path) out.push(path);
  }
  return out;
}

function clampInt(value: number, min: number, max: number): number {
  if (!Number.isFinite(value)) return min;
  return Math.min(max, Math.max(min, Math.round(value)));
}

function clampNum(value: number, min: number, max: number, fallback: number): number {
  if (!Number.isFinite(value)) return fallback;
  return Math.min(max, Math.max(min, value));
}

const THEME_CACHE_KEY = "unparanoia.theme";

function systemTheme(): "light" | "dark" {
  try {
    return window.matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light";
  } catch {
    return "dark";
  }
}

function cachedTheme(): "light" | "dark" | null {
  try {
    const value = window.localStorage.getItem(THEME_CACHE_KEY);
    return value === "light" || value === "dark" ? value : null;
  } catch {
    return null;
  }
}

function applyTheme(theme: ThemeMode): "light" | "dark" {
  const resolved: "light" | "dark" = theme === "system" ? systemTheme() : theme;
  if (typeof document !== "undefined") {
    document.documentElement.dataset.theme = resolved;
  }
  try {
    window.localStorage.setItem(THEME_CACHE_KEY, resolved);
  } catch {
    /* localStorage 不可用时忽略 */
  }
  return resolved;
}

export function bootstrapTheme(): void {
  if (typeof document === "undefined") return;
  applyTheme(cachedTheme() ?? "system");
}

function themeLabel(theme: ThemeMode): string {
  switch (theme) {
    case "light":
      return "浅色";
    case "dark":
      return "深色";
    default:
      return "跟随系统";
  }
}

function normalizeSettings(raw: Partial<Settings> | null | undefined): Settings {
  const merged: Settings = { ...DEFAULT_SETTINGS, ...(raw ?? {}) };
  const theme = merged.theme;
  return {
    ...merged,
    theme: theme === "light" || theme === "dark" || theme === "system" ? theme : "system",
  };
}

function maskPassword(value: string): string {
  const count = Math.min(Math.max(value.length, 4), 12);
  return "•".repeat(count);
}

/* ================= 任务行 / 计数 ================= */

interface Task {
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

const REQUEUE: Partial<Task> = {
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

type LiveCounts = Omit<Summary, "elapsed_ms">;

const EMPTY_COUNTS: LiveCounts = {
  total: 0,
  success: 0,
  skipped: 0,
  needs_password: 0,
  failed: 0,
  cancelled: 0,
  bytes: 0,
};

type Tab = "queue" | "logs" | "passwords" | "settings";

const TABS: { id: Tab; label: string }[] = [
  { id: "queue", label: "队列" },
  { id: "logs", label: "日志" },
  { id: "passwords", label: "密码" },
  { id: "settings", label: "设置" },
];

/* ================= 主组件 ================= */

export default function App() {
  const [info, setInfo] = useState<AppInfo | null>(null);
  const [settings, setSettings] = useState<Settings>(DEFAULT_SETTINGS);
  const [configPath, setConfigPath] = useState("");
  const [password, setPassword] = useState("");
  const [rememberPassword, setRememberPassword] = useState(false);
  const [passwords, setPasswords] = useState<string[]>([]);
  const [pwRevealed, setPwRevealed] = useState<number[]>([]);
  const [pwInput, setPwInput] = useState("");
  const [iconOk, setIconOk] = useState(true);
  const [tasks, setTasks] = useState<Task[]>([]);
  const [logs, setLogs] = useState<string[]>([]);
  const [running, setRunning] = useState(false);
  const [paused, setPaused] = useState(false);
  const [summary, setSummary] = useState<Summary | null>(null);
  const [live, setLive] = useState<LiveCounts>({ ...EMPTY_COUNTS });
  const [batchKeys, setBatchKeys] = useState<string[]>([]);
  const [skipNotice, setSkipNotice] = useState<string[] | null>(null);
  const [activity, setActivity] = useState("");
  const [dragging, setDragging] = useState(false);
  const [tab, setTab] = useState<Tab>("queue");
  const [toast, setToast] = useState<string | null>(null);

  const logBox = useRef<HTMLDivElement | null>(null);
  const idKey = useRef<Map<number, string>>(new Map());
  const tasksRef = useRef<Task[]>([]);
  const batchKeysRef = useRef<string[]>([]);

  useEffect(() => {
    tasksRef.current = tasks;
  }, [tasks]);

  /* ---------- 基础回调 ---------- */

  const pushLog = useCallback((line: string) => {
    const stamp = new Date().toLocaleTimeString("zh-CN", { hour12: false });
    setLogs((prev) => {
      const next = prev.concat("[" + stamp + "] " + line);
      return next.length > 2000 ? next.slice(next.length - 2000) : next;
    });
    void invoke("append_log", { line }).catch(() => {});
  }, []);

  const patchByKey = useCallback((key: string, patch: Partial<Task>) => {
    setTasks((prev) => {
      const idx = prev.findIndex((t) => t.key === key);
      if (idx < 0) {
        const created: Task = {
          key,
          path: key,
          name: basename(key),
          size: null,
          phase: "queued",
          ...patch,
        };
        return prev.concat(created);
      }
      const next = prev.slice();
      next[idx] = { ...next[idx], ...patch };
      return next;
    });
  }, []);

  const patchById = useCallback(
    (id: number, patch: Partial<Task>) => {
      const key = idKey.current.get(id);
      if (key) patchByKey(key, patch);
    },
    [patchByKey],
  );

  const addPaths = useCallback((paths: string[]): { added: number; requeued: number } => {
    const unique = Array.from(new Set(paths.map((p) => p.trim()).filter((p) => p.length > 0)));
    if (unique.length === 0) return { added: 0, requeued: 0 };
    const known = new Map(tasksRef.current.map((t) => [t.key, t]));
    let added = 0;
    let requeued = 0;
    const toAdd: Task[] = [];
    const toRequeue = new Set<string>();
    for (const path of unique) {
      const current = known.get(path);
      if (!current) {
        toAdd.push({ key: path, path, name: basename(path), size: null, phase: "queued" });
        added += 1;
      } else if (current.phase === "done") {
        toRequeue.add(path);
        requeued += 1;
      }
    }
    if (toAdd.length === 0 && toRequeue.size === 0) return { added, requeued };
    setTasks((prev) => {
      const existing = new Set(prev.map((t) => t.key));
      const next = prev.map((t) => (toRequeue.has(t.key) ? { ...t, ...REQUEUE } : t));
      const adds = toAdd.filter((t) => !existing.has(t.key));
      return adds.length > 0 ? next.concat(adds) : next;
    });
    return { added, requeued };
  }, []);

  const addRaw = useCallback(
    async (raw: string[], source: string) => {
      if (raw.length === 0) return;
      try {
        const expanded = asPathList(await invoke<unknown>("expand_paths", { paths: raw }));
        if (expanded.length === 0) {
          setToast("没有可加入的文件（路径不存在或已被输出根目录排除）");
          pushLog(source + "：展开后为空（原始 " + raw.length + " 个路径）");
          return;
        }
        const result = addPaths(expanded);
        if (result.added === 0 && result.requeued === 0) {
          setToast("没有新增文件（都已在列表中）");
          pushLog(source + "：展开 " + expanded.length + " 个文件，均已在列表中");
          return;
        }
        setTab("queue");
        setToast(
          "已加入 " + result.added + " 个文件" + (result.requeued > 0 ? "，重新排队 " + result.requeued + " 个" : ""),
        );
        pushLog(
          source +
            "：展开 " +
            expanded.length +
            " 个文件，新增 " +
            result.added +
            " 个" +
            (result.requeued > 0 ? "，重新排队 " + result.requeued + " 个" : ""),
        );
      } catch (error) {
        const text = errText(error);
        pushLog("展开路径失败: " + text);
        setToast("展开路径失败: " + text);
      }
    },
    [addPaths, pushLog],
  );

  const refreshInfo = useCallback(async () => {
    try {
      setInfo(await invoke<AppInfo>("app_info"));
    } catch (error) {
      pushLog("app_info 失败: " + errText(error));
    }
  }, [pushLog]);

  /* ---------- 密码库 ---------- */

  const loadPasswords = useCallback(async () => {
    try {
      const list = await invoke<string[]>("list_passwords");
      setPasswords(Array.isArray(list) ? list : []);
    } catch (error) {
      pushLog("读取密码库失败: " + errText(error));
    }
  }, [pushLog]);

  const addPassword = useCallback(async () => {
    const value = pwInput.trim();
    if (value.length === 0) {
      setToast("密码不能为空");
      return;
    }
    try {
      const added = await invoke<boolean>("add_password", { password: value });
      if (added) {
        pushLog("已添加密码到密码库（明文保存）");
        setToast("已添加到密码库");
      } else {
        setToast("该密码已在密码库中");
      }
      setPwInput("");
      await loadPasswords();
    } catch (error) {
      setToast("添加密码失败: " + errText(error));
    }
  }, [pwInput, loadPasswords, pushLog]);

  const removePassword = useCallback(
    async (index: number) => {
      try {
        const removed = await invoke<boolean>("remove_password", { index });
        if (removed) {
          pushLog("已删除密码库第 " + (index + 1) + " 条（明文）");
          setToast("已删除该密码");
        } else {
          setToast("密码不存在，密码库可能已变化");
        }
        setPwRevealed((prev) => prev.filter((value) => value !== index));
        await loadPasswords();
      } catch (error) {
        setToast("删除密码失败: " + errText(error));
      }
    },
    [loadPasswords, pushLog],
  );

  const clearPasswords = useCallback(async () => {
    if (passwords.length === 0) return;
    const ok = await confirmDialog("清空密码库？所有保存的密码都会从配置文件中删除。", {
      title: "清空密码库",
      kind: "warning",
    });
    if (!ok) return;
    try {
      await invoke("clear_passwords");
      setPwRevealed([]);
      await loadPasswords();
      pushLog("已清空密码库");
      setToast("已清空密码库");
    } catch (error) {
      setToast("清空密码库失败: " + errText(error));
    }
  }, [passwords.length, loadPasswords, pushLog]);

  const togglePwReveal = useCallback((index: number) => {
    setPwRevealed((prev) =>
      prev.includes(index) ? prev.filter((value) => value !== index) : prev.concat(index),
    );
  }, []);

  /* ---------- 外观主题 ---------- */

  const changeTheme = useCallback(
    async (theme: ThemeMode) => {
      const next: Settings = { ...settings, theme };
      setSettings(next);
      applyTheme(theme);
      try {
        await invoke("settings_set", { settings: next });
        pushLog("外观主题已切换为" + themeLabel(theme) + "（已保存）");
      } catch (error) {
        const text = errText(error);
        pushLog("保存主题失败: " + text);
        setToast("保存主题失败: " + text);
      }
    },
    [settings, pushLog],
  );

  /* ---------- 批次控制 ---------- */

  const startPaths = useCallback(
    async (paths: string[], override?: Partial<Settings>) => {
      const files = Array.from(new Set(paths));
      if (files.length === 0) {
        setToast("先添加文件或文件夹");
        return;
      }
      if (running) {
        setToast("已有批次在运行，请等待结束");
        return;
      }
      const effective: Settings = { ...settings, ...(override ?? {}) };
      const trimmed = password.trim();
      setSummary(null);
      setLive({ ...EMPTY_COUNTS });
      setBatchKeys(files);
      batchKeysRef.current = files;
      setSkipNotice(null);
      setActivity("");
      setRunning(true);
      setPaused(false);
      idKey.current.clear();
      setTasks((prev) => prev.map((t) => (files.includes(t.key) ? { ...t, ...REQUEUE } : t)));
      const req: StartRequest = {
        files,
        password: trimmed.length > 0 ? trimmed : null,
        rememberPassword,
        settings: effective,
      };
      setTab("queue");
      pushLog(
        "开始批次：共 " +
          files.length +
          " 个文件" +
          (trimmed.length > 0 ? "（含本次追加密码）" : "") +
          (rememberPassword && trimmed.length > 0 ? "，并记住到密码库（明文保存）" : ""),
      );
      try {
        await invoke("start_batch", { req });
        if (rememberPassword && trimmed.length > 0) {
          setPassword("");
          setRememberPassword(false);
          pushLog("本次追加密码已记住到密码库（明文保存）");
          await loadPasswords();
        }
      } catch (error) {
        const text = errText(error);
        setRunning(false);
        setPaused(false);
        setActivity("");
        pushLog("启动失败: " + text);
        setToast("启动失败: " + text);
      }
    },
    [running, settings, password, rememberPassword, pushLog, loadPasswords],
  );

  const queuedTasks = useMemo(() => tasks.filter((t) => t.phase === "queued"), [tasks]);
  const retryableTasks = useMemo(
    () => tasks.filter((t) => t.phase === "done" && (t.status === "failed" || t.status === "needs_password")),
    [tasks],
  );

  const start = useCallback(() => {
    if (queuedTasks.length === 0) {
      setToast("没有待处理的文件");
      return;
    }
    void startPaths(queuedTasks.map((t) => t.path));
  }, [queuedTasks, startPaths]);

  const pauseBatch = useCallback(async () => {
    try {
      await invoke("pause_batch");
      setPaused(true);
      pushLog("已请求暂停（当前任务完成后暂停）");
    } catch (error) {
      setToast("暂停失败: " + errText(error));
    }
  }, [pushLog]);

  const resumeBatch = useCallback(async () => {
    try {
      await invoke("resume_batch");
      setPaused(false);
      pushLog("已继续批次");
    } catch (error) {
      setToast("继续失败: " + errText(error));
    }
  }, [pushLog]);

  const cancelBatch = useCallback(async () => {
    try {
      await invoke("cancel_batch");
      setPaused(false);
      pushLog("已请求取消批次…");
    } catch (error) {
      setToast("取消失败: " + errText(error));
    }
  }, [pushLog]);

  const retryOne = useCallback(
    (path: string, override?: Partial<Settings>) => {
      if (running) {
        setToast("当前批次仍在运行");
        return;
      }
      void startPaths([path], override);
    },
    [running, startPaths],
  );

  const retryFailed = useCallback(() => {
    if (running) {
      setToast("当前批次仍在运行");
      return;
    }
    if (retryableTasks.length === 0) {
      setToast("没有失败或需密码的条目");
      return;
    }
    void startPaths(retryableTasks.map((t) => t.path));
  }, [running, retryableTasks, startPaths]);

  const removeTask = useCallback((key: string) => {
    setTasks((prev) => prev.filter((t) => t.key !== key));
  }, []);

  const reveal = useCallback(async (path: string | null | undefined) => {
    if (!path) return;
    try {
      await invoke("reveal_path", { path });
    } catch (error) {
      setToast("定位失败: " + errText(error));
    }
  }, []);

  const inspectFile = useCallback(
    async (path: string) => {
      try {
        const detection = await invoke<Detection>("inspect_file", { path });
        patchByKey(path, { detection, size: detection.size });
        const notes = detection.notes.join("；");
        pushLog(
          "探测 " +
            basename(path) +
            "：" +
            kindLabel(detection.kind) +
            (detection.format ? " / " + detection.format : "") +
            " · " +
            humanBytes(detection.size) +
            " · " +
            passwordStateLabel(detection.password_state) +
            (notes ? " · " + notes : ""),
        );
        setToast(basename(path) + "：" + kindLabel(detection.kind) + " · " + passwordStateLabel(detection.password_state));
      } catch (error) {
        setToast("探测失败: " + errText(error));
      }
    },
    [patchByKey, pushLog],
  );

  const clearList = useCallback(async () => {
    if (tasks.length === 0) return;
    if (running || tasks.some((t) => t.phase === "done")) {
      const ok = await confirmDialog("清空任务列表？已写入磁盘的文件不受影响。", {
        title: "清空列表",
        kind: "warning",
      });
      if (!ok) return;
    }
    idKey.current.clear();
    setTasks([]);
    setSummary(null);
    setLive({ ...EMPTY_COUNTS });
    setBatchKeys([]);
    batchKeysRef.current = [];
    setSkipNotice(null);
    setActivity("");
    pushLog("已清空任务列表");
  }, [tasks, running, pushLog]);

  const saveSettings = useCallback(
    async (next: Settings) => {
      try {
        await invoke("settings_set", { settings: next });
        setSettings(next);
        pushLog("设置已保存");
        setToast("设置已保存");
        await refreshInfo();
      } catch (error) {
        const text = errText(error);
        setToast("保存设置失败: " + text);
        pushLog("保存设置失败: " + text);
      }
    },
    [pushLog, refreshInfo],
  );

  const cleanupStaging = useCallback(
    async (outputRoot: string | null) => {
      const roots = outputRoot && outputRoot.trim().length > 0 ? [outputRoot.trim()] : [];
      try {
        const deleted = await invoke<number>("cleanup_staging", { roots });
        const where = roots.length > 0 ? roots[0] : "系统临时目录";
        pushLog("清理临时残留：删除 " + deleted + " 个残留（扫描 " + where + "）");
        setToast("已清理 " + deleted + " 个临时残留");
      } catch (error) {
        const text = errText(error);
        pushLog("清理临时残留失败: " + text);
        setToast("清理失败: " + text);
      }
    },
    [pushLog],
  );

  const forgetProcessed = useCallback(async () => {
    const ok = await confirmDialog(
      "清除处理记录？删除 processed.json 后，已成功解压过的文件不会再被跳过（会重新解压）。",
      { title: "清除处理记录", kind: "warning" },
    );
    if (!ok) return;
    try {
      const removed = await invoke<number>("forget_processed");
      pushLog("清除处理记录：" + removed + " 条");
      setToast(removed > 0 ? "已清除 " + removed + " 条处理记录" : "没有处理记录");
    } catch (error) {
      setToast("清除失败: " + errText(error));
    }
  }, [pushLog]);

  const copyLogs = useCallback(async () => {
    try {
      await navigator.clipboard.writeText(logs.join(String.fromCharCode(10)));
      setToast("日志已复制到剪贴板");
    } catch (error) {
      setToast("复制失败: " + errText(error));
    }
  }, [logs]);

  const revealLogDir = useCallback(async () => {
    let dir: string | null = null;
    if (info) {
      if (info.logDir) {
        dir = info.logDir;
      } else if (info.configDir) {
        const sep = info.configDir.includes(BACKSLASH) ? BACKSLASH : "/";
        dir = info.configDir.replace(/[\\/]+$/, "") + sep + "logs";
      }
    }
    if (!dir) {
      setToast("日志目录未知（后端尚未提供 logDir）");
      return;
    }
    try {
      await invoke("reveal_path", { path: dir });
    } catch (error) {
      setToast("打开日志目录失败: " + errText(error));
    }
  }, [info]);

  const pickInput = useCallback(
    async (mode: "files" | "folder") => {
      try {
        let raw: string[] = [];
        if (mode === "files") {
          try {
            raw = asPathList(await invoke<unknown>("pick_input_files"));
          } catch (error) {
            pushLog("pick_input_files 调用失败，改用前端对话框: " + errText(error));
            const picked = await openDialog({ multiple: true, directory: false, title: "选择要解密/解压的文件" });
            raw = Array.isArray(picked) ? picked : picked ? [picked] : [];
          }
        } else {
          const picked = await openDialog({ multiple: false, directory: true, title: "选择文件夹（递归展开）" });
          raw = Array.isArray(picked) ? picked : picked ? [picked] : [];
        }
        if (raw.length === 0) return;
        await addRaw(raw, mode === "files" ? "选择文件" : "选择文件夹");
      } catch (error) {
        setToast("选择失败: " + errText(error));
      }
    },
    [addRaw, pushLog],
  );

  /* ---------- 启动：app_info → settings_get → list_passwords → startup_paths → expand_paths ---------- */

  useEffect(() => {
    let disposed = false;
    (async () => {
      try {
        const appInfo = await invoke<AppInfo>("app_info");
        if (!disposed) setInfo(appInfo);
      } catch (error) {
        pushLog("app_info 失败: " + errText(error));
      }
      try {
        const loaded = normalizeSettings(await invoke<Partial<Settings>>("settings_get"));
        if (!disposed) setSettings(loaded);
      } catch (error) {
        pushLog("settings_get 失败: " + errText(error));
      }
      try {
        const list = await invoke<string[]>("list_passwords");
        if (!disposed) setPasswords(Array.isArray(list) ? list : []);
      } catch (error) {
        pushLog("读取密码库失败: " + errText(error));
      }
      try {
        const path = await invoke<string>("get_config_path");
        if (!disposed) setConfigPath(path);
      } catch (error) {
        pushLog("读取配置文件路径失败: " + errText(error));
      }
      try {
        const startup = asPathList(await invoke<unknown>("startup_paths"));
        if (!disposed && startup.length > 0) {
          pushLog("启动参数包含 " + startup.length + " 个路径");
          await addRaw(startup, "启动参数");
        }
      } catch (error) {
        pushLog("读取启动参数失败: " + errText(error));
      }
      try {
        const state = await invoke<BatchState>("batch_state");
        if (!disposed) {
          if (state.running) {
            setRunning(true);
            setPaused(state.paused);
            pushLog("检测到已有批次在运行");
          }
          if (state.summary) setSummary(state.summary);
        }
      } catch {
        /* batch_state 失败不阻塞启动 */
      }
    })();
    return () => {
      disposed = true;
    };
  }, [addRaw, pushLog]);

  /* ---------- 外观主题：system 时跟随系统，light/dark 显式覆盖 ---------- */

  useEffect(() => {
    applyTheme(settings.theme);
    if (settings.theme !== "system" || typeof window.matchMedia !== "function") return;
    const media = window.matchMedia("(prefers-color-scheme: dark)");
    const onChange = () => applyTheme("system");
    media.addEventListener("change", onChange);
    return () => media.removeEventListener("change", onChange);
  }, [settings.theme]);

  /* ---------- 后端事件：unparanoia://event / unparanoia://done ---------- */

  useEffect(() => {
    let disposed = false;
    const unsubs: Array<() => void> = [];

    (async () => {
      const unlistenEvent = await listen<TaskEvent>("unparanoia://event", (event) => {
        const ev = event.payload;
        switch (ev.type) {
          case "task_queued": {
            idKey.current.set(ev.id, ev.path);
            patchByKey(ev.path, { id: ev.id, size: ev.size, phase: "queued" });
            break;
          }
          case "task_stage": {
            patchById(ev.id, { stage: ev.stage, stageDetail: ev.detail ?? null, phase: "running" });
            const key = idKey.current.get(ev.id);
            const detail = ev.detail ? " · " + ev.detail : "";
            setActivity((key ? basename(key) + " · " : "") + stageLabel(ev.stage) + detail);
            break;
          }
          case "task_detected": {
            patchById(ev.id, { detection: ev.detection, size: ev.detection.size });
            const key = idKey.current.get(ev.id);
            pushLog(
              "识别 " +
                (key ? basename(key) : "#" + ev.id) +
                "：" +
                kindLabel(ev.detection.kind) +
                (ev.detection.format ? " / " + ev.detection.format : "") +
                " · " +
                humanBytes(ev.detection.size) +
                " · " +
                passwordStateLabel(ev.detection.password_state),
            );
            break;
          }
          case "task_progress": {
            patchById(ev.id, { percent: ev.percent, line: ev.line ?? null, phase: "running" });
            if (ev.line) {
              const key = idKey.current.get(ev.id);
              setActivity((key ? basename(key) + " · " : "") + ev.line);
            }
            break;
          }
          case "task_password_attempt": {
            pushLog("#" + ev.id + " 尝试密码（" + ev.source + "）：" + (ev.ok ? "通过" : "失败") + "，第 " + ev.tried + " 次");
            break;
          }
          case "task_password_missing": {
            pushLog("#" + ev.id + " 没有可用密码（已尝试 " + ev.tried + " 个）");
            break;
          }
          case "task_finished": {
            const outcome = ev.outcome;
            setLive((prev) => {
              const next: LiveCounts = {
                ...prev,
                total: prev.total + 1,
                bytes: prev.bytes + (outcome.bytes || 0),
              };
              switch (outcome.status) {
                case "success":
                  next.success += 1;
                  break;
                case "skipped":
                  next.skipped += 1;
                  break;
                case "needs_password":
                  next.needs_password += 1;
                  break;
                case "failed":
                  next.failed += 1;
                  break;
                case "cancelled":
                  next.cancelled += 1;
                  break;
                default:
                  next.failed += 1;
                  break;
              }
              return next;
            });
            const patch: Partial<Task> = {
              phase: "done",
              status: outcome.status,
              dest: outcome.dest ?? null,
              reason: outcome.reason ?? null,
              warnings: outcome.warnings ?? [],
              warningsOpen: false,
              files: outcome.files,
              bytes: outcome.bytes,
              elapsed_ms: outcome.elapsed_ms,
              password_source: outcome.password_source ?? null,
              percent: outcome.status === "success" || outcome.status === "skipped" ? 100 : null,
            };
            if (outcome.detection) patch.detection = outcome.detection;
            patchById(ev.id, patch);
            const related = outcome.related_paths ?? [];
            if (related.length > 0) {
              const mergedReason = "已并入 " + basename(outcome.path) + "（分卷）";
              const relatedPatch: Partial<Task> = {
                phase: "done",
                status: outcome.status,
                percent: outcome.status === "success" || outcome.status === "skipped" ? 100 : null,
                reason: mergedReason,
              };
              const knownKeys = new Set(tasksRef.current.map((t) => t.key));
              for (const rel of related) {
                if (!rel || rel === outcome.path) continue;
                if (!knownKeys.has(rel)) continue;
                patchByKey(rel, relatedPatch);
              }
            }
            if (outcome.status === "failed" || outcome.status === "needs_password") {
              pushLog(basename(outcome.path) + " " + statusLabel(outcome.status) + "：" + (outcome.reason ?? "无详情"));
            }
            break;
          }
          case "log": {
            pushLog(ev.line);
            break;
          }
          default:
            break;
        }
      });
      if (disposed) {
        unlistenEvent();
        return;
      }
      unsubs.push(unlistenEvent);

      const unlistenDone = await listen<Summary>("unparanoia://done", (event) => {
        const done = event.payload;
        const batch = batchKeysRef.current;
        // 正常情况用本批 keys；若应用在批次运行中重启（batchKeys 丢失），退化为
        // 当前仍在排队/运行的行，避免这些行永远停在「待处理」。
        const settleKeys =
          batch.length > 0
            ? batch
            : tasksRef.current
                .filter((t) => t.phase === "queued" || t.phase === "running")
                .map((t) => t.key);
        setSummary(done);
        setRunning(false);
        setPaused(false);
        setActivity("");
        // 仅在正常跑完（无失败/取消）时收尾；取消或 stopOnError 中止时保留未开始
        // 的行，方便用户直接重试，避免被误标为「分卷跳过」。
        const normalDone = done.cancelled === 0 && done.failed === 0;
        if (normalDone && settleKeys.length > 0) {
          const keys = new Set(settleKeys);
          setTasks((prev) =>
            prev.map((t): Task => {
              if (!keys.has(t.key) || (t.phase !== "queued" && t.phase !== "running")) return t;
              return {
                ...t,
                phase: "done",
                status: "skipped",
                percent: 100,
                reason: "未处理：同一分卷任务已由首卷处理",
              };
            }),
          );
        }
        pushLog(
          "批次完成：成功 " +
            done.success +
            " / 跳过 " +
            done.skipped +
            " / 需密码 " +
            done.needs_password +
            " / 失败 " +
            done.failed +
            " / 取消 " +
            done.cancelled +
            "，共 " +
            humanBytes(done.bytes) +
            "，用时 " +
            (done.elapsed_ms / 1000).toFixed(1) +
            " 秒",
        );
        if (normalDone && done.success === 0 && done.skipped > 0 && done.needs_password === 0) {
          const message = "文件因历史处理记录被跳过：可点「重新解压」，或在设置中清除处理记录";
          pushLog(message);
          setToast(message);
          setSkipNotice(settleKeys.length > 0 ? settleKeys : null);
        } else {
          setSkipNotice(null);
        }
        void refreshInfo();
      });
      if (disposed) {
        unlistenDone();
        return;
      }
      unsubs.push(unlistenDone);
    })().catch((error: unknown) => {
      pushLog("事件监听失败: " + errText(error));
    });

    return () => {
      disposed = true;
      for (const unlisten of unsubs) unlisten();
    };
  }, [patchById, patchByKey, pushLog, refreshInfo]);

  /* ---------- Tauri 拖放 ---------- */

  useEffect(() => {
    let disposed = false;
    const holder: { unlisten: null | (() => void) } = { unlisten: null };
    getCurrentWebview()
      .onDragDropEvent((event) => {
        const payload = event.payload;
        if (payload.type === "over" || payload.type === "enter") {
          setDragging(true);
          return;
        }
        if (payload.type === "leave") {
          setDragging(false);
          return;
        }
        if (payload.type === "drop") {
          setDragging(false);
          if (payload.paths.length === 0) return;
          void addRaw(payload.paths, "拖放");
        }
      })
      .then((unlisten) => {
        if (disposed) unlisten();
        else holder.unlisten = unlisten;
      })
      .catch((error: unknown) => {
        pushLog("拖放监听失败: " + errText(error));
      });
    return () => {
      disposed = true;
      if (holder.unlisten) holder.unlisten();
    };
  }, [addRaw, pushLog]);

  /* ---------- toast 自动消失 / 日志滚动 ---------- */

  useEffect(() => {
    if (!toast) return;
    const timer = window.setTimeout(() => setToast(null), 3500);
    return () => window.clearTimeout(timer);
  }, [toast]);

  useEffect(() => {
    if (logBox.current) logBox.current.scrollTop = logBox.current.scrollHeight;
  }, [logs, tab]);

  /* ---------- 批次状态轮询兜底（事件丢失时恢复按钮可用性） ---------- */

  useEffect(() => {
    if (!running) return;
    let cancelled = false;
    const reconcile = async () => {
      try {
        const state = await invoke<BatchState>("batch_state");
        if (cancelled) return;
        if (!state.running) {
          setRunning(false);
          setPaused(false);
          setActivity("");
          if (state.summary) setSummary(state.summary);
          pushLog("批次已结束（状态轮询兜底）");
          void refreshInfo();
        }
      } catch {
        /* 忽略临时错误 */
      }
    };
    const timer = window.setInterval(() => {
      void reconcile();
    }, 2000);
    const onFocus = () => {
      void reconcile();
    };
    window.addEventListener("focus", onFocus);
    return () => {
      cancelled = true;
      window.clearInterval(timer);
      window.removeEventListener("focus", onFocus);
    };
  }, [running, pushLog, refreshInfo]);

  /* ---------- 派生数据 ---------- */

  const counts: LiveCounts = summary
    ? {
        total: summary.total,
        success: summary.success,
        skipped: summary.skipped,
        needs_password: summary.needs_password,
        failed: summary.failed,
        cancelled: summary.cancelled,
        bytes: summary.bytes,
      }
    : live;

  const batchTasks = useMemo(() => {
    if (batchKeys.length === 0) return [];
    const keys = new Set(batchKeys);
    return tasks.filter((t) => keys.has(t.key));
  }, [batchKeys, tasks]);

  const batchProgress = useMemo(() => {
    if (batchTasks.length === 0) return { percent: 0, known: false };
    const doneCount = batchTasks.filter((t) => t.phase === "done").length;
    if (doneCount === batchTasks.length) return { percent: 100, known: true };
    const anyPercent = batchTasks.some((t) => t.phase === "done" || t.percent != null);
    const sum = batchTasks.reduce((acc, t) => acc + (t.phase === "done" ? 100 : t.percent ?? 0), 0);
    if (anyPercent) return { percent: Math.min(100, Math.round(sum / batchTasks.length)), known: true };
    const finished = summary ? Math.min(summary.total, batchTasks.length) : 0;
    return {
      percent: Math.min(100, Math.round((finished / batchTasks.length) * 100)),
      known: finished > 0,
    };
  }, [batchTasks, summary]);

  const queuedSize = queuedTasks.reduce((acc, t) => acc + (t.size ?? 0), 0);
  const recentLog = logs.length > 0 ? logs[logs.length - 1] : "";

  return (
    <div className={"app" + (dragging ? " dragging" : "")}>
      <header className="topbar">
        <div className="brand">
          {iconOk ? (
            <img
              className="appicon"
              src="/icon.png"
              alt="Unparanoia"
              width={28}
              height={28}
              onError={() => setIconOk(false)}
            />
          ) : (
            <span className="logo-fallback" aria-hidden="true">
              U
            </span>
          )}
          <div>
            <div className="title">Unparanoia</div>
            <div className="subtitle">帕罗诺亚解包器 · SSEFE .enc 解密 · 伪装压缩包 / 自解压 EXE 批量解压</div>
          </div>
        </div>
        <div className="topinfo">
          <span>v{info ? info.version : "0.3.0"}</span>
          <span className="dot" />
          <span title={info && info.sevenZip ? info.sevenZip : "未找到 7-Zip"}>
            7-Zip {info ? info.sevenZipVersion ?? "未找到" : "…"}
          </span>
        </div>
      </header>

      <section className="pwbar">
        <label className="pwlabel" htmlFor="pw-input">
          本次追加密码（仅本次运行）
        </label>
        <input
          id="pw-input"
          className="pwinput"
          type="password"
          autoComplete="off"
          spellCheck={false}
          placeholder="可选；留空则只使用密码库中的密码"
          value={password}
          onChange={(event) => setPassword(event.target.value)}
          onKeyDown={(event) => {
            if (event.key === "Enter") start();
          }}
        />
        <label className="check" title="勾选后，本次追加密码会明文保存到本机密码库">
          <input
            type="checkbox"
            checked={rememberPassword}
            onChange={(event) => setRememberPassword(event.target.checked)}
          />
          记住到密码库（明文保存）
        </label>
        <button className="mini" onClick={() => setTab("passwords")} title="打开密码管理页">
          密码库 {passwords.length}
        </button>
        <span className="spacer" />
        <span className="muted mono-ellipsis" title={configPath || (info ? info.configPath : "")}>
          配置文件：{configPath || (info ? info.configPath : "") || "—"}
        </span>
      </section>

      <nav className="tabs">
        {TABS.map((item) => (
          <button
            key={item.id}
            className={"tab" + (tab === item.id ? " active" : "")}
            onClick={() => setTab(item.id)}
          >
            {item.label}
          </button>
        ))}
        <span className="spacer" />
        {info && !info.sevenZip && <span className="warn-inline">未找到 7-Zip，解压/识别可能不可用</span>}
      </nav>

      {tab === "queue" && (
        <main className="page">
          <section className={"dropzone compact" + (dragging ? " hot" : "")}>
            <div className="dz-main">
              <div className="dz-title">{dragging ? "松开鼠标加入队列" : "把文件或文件夹拖到这里"}</div>
              <div className="dz-sub">
                支持 .enc 加密容器、普通/伪装压缩包（如 .mp4）、自解压 EXE；文件夹会递归展开
              </div>
            </div>
            <div className="dz-actions">
              <button onClick={() => void pickInput("files")} disabled={running}>
                选择文件
              </button>
              <button onClick={() => void pickInput("folder")} disabled={running}>
                选择文件夹
              </button>
            </div>
          </section>

          {skipNotice && (
            <div className="skipnotice">
              <span>
                文件因历史处理记录被跳过：可点「重新解压」，或在设置中清除处理记录。
              </span>
              <span className="spacer" />
              <button
                className="mini"
                onClick={() => {
                  void forgetProcessed();
                  setSkipNotice(null);
                }}
                disabled={running}
              >
                清除处理记录
              </button>
              <button
                className="mini primary"
                onClick={() => void startPaths(skipNotice, { skipProcessed: false })}
                disabled={running}
              >
                全部重新解压
              </button>
            </div>
          )}

          <section className="panel grow">
            <div className="toolbar">
              <button className="primary" onClick={start} disabled={running || queuedTasks.length === 0}>
                开始{queuedTasks.length > 0 ? " (" + queuedTasks.length + ")" : ""}
              </button>
              {running ? (
                <>
                  {paused ? (
                    <button onClick={() => void resumeBatch()}>继续</button>
                  ) : (
                    <button onClick={() => void pauseBatch()}>暂停</button>
                  )}
                  <button className="danger" onClick={() => void cancelBatch()}>
                    取消
                  </button>
                </>
              ) : null}
              <button
                onClick={retryFailed}
                disabled={running || retryableTasks.length === 0}
                title="用列表中的失败/需密码项重新开始批次（密码取输入框）"
              >
                重试失败/需密码{retryableTasks.length > 0 ? " (" + retryableTasks.length + ")" : ""}
              </button>
              <button onClick={() => void clearList()} disabled={running || tasks.length === 0}>
                清空列表
              </button>
              <span className="spacer" />
              <span className="muted toolbar-hint">
                {settings.outputRoot ? "输出根目录：" + settings.outputRoot : "输出根目录：压缩包所在目录"} · 并发{" "}
                {settings.jobs}
              </span>
            </div>

            {(running || summary) && (
              <div className="batchprog">
                <div className="progress big" title={batchProgress.known ? batchProgress.percent + "%" : "进度未知"}>
                  <div className="bar" style={{ width: batchProgress.percent + "%" }} />
                  <span className="pct">{batchProgress.known ? batchProgress.percent + "%" : "—"}</span>
                </div>
                <span className="muted activity" title={activity}>
                  {running ? (paused ? "已暂停" : activity || "处理中…") : "批次已结束"}
                </span>
              </div>
            )}

            {(summary || live.total > 0) && (
              <div className="summarybar">
                <span className="sumtitle">{summary ? "本次批次汇总" : "实时统计"}</span>
                <span className="ok">成功 {counts.success}</span>
                <span>跳过 {counts.skipped}</span>
                <span className="warn">需密码 {counts.needs_password}</span>
                <span className={counts.failed > 0 ? "bad" : ""}>失败 {counts.failed}</span>
                <span>取消 {counts.cancelled}</span>
                <span className="muted">
                  共 {counts.total} 项 · {humanBytes(counts.bytes)}
                  {summary ? " · 用时 " + (summary.elapsed_ms / 1000).toFixed(1) + "s" : ""}
                </span>
              </div>
            )}

            <div className="tablewrap">
              <table className="queue">
                <thead>
                  <tr>
                    <th className="col-name">文件</th>
                    <th>类型</th>
                    <th className="col-size">大小</th>
                    <th>密码状态</th>
                    <th>状态</th>
                    <th className="col-prog">进度</th>
                    <th>结果</th>
                    <th className="col-actions">操作</th>
                  </tr>
                </thead>
                <tbody>
                  {tasks.length === 0 && (
                    <tr>
                      <td className="empty" colSpan={8}>
                        队列为空 —— 把文件或文件夹拖进来即可开始
                      </td>
                    </tr>
                  )}
                  {tasks.map((task) => (
                    <TaskRow
                      key={task.key}
                      task={task}
                      running={running}
                      onRetry={retryOne}
                      onReextract={(path) => retryOne(path, { skipProcessed: false })}
                      onReveal={reveal}
                      onInspect={inspectFile}
                      onRemove={removeTask}
                      onToggleWarnings={(key) => {
                        setTasks((prev) =>
                          prev.map((t) => (t.key === key ? { ...t, warningsOpen: !t.warningsOpen } : t)),
                        );
                      }}
                    />
                  ))}
                </tbody>
              </table>
            </div>

            <div className="recentbar">
              <span className="muted">
                {queuedTasks.length > 0
                  ? "待处理 " + queuedTasks.length + " 项 / 共 " + humanBytes(queuedSize)
                  : "没有待处理文件"}
              </span>
              <span className="spacer" />
              <span className="muted mono-ellipsis" title={recentLog}>
                {recentLog || "最近日志：—"}
              </span>
            </div>
          </section>
        </main>
      )}

      {tab === "logs" && (
        <main className="page">
          <section className="panel grow">
            <div className="panel-head">
              <h3>日志</h3>
              <div className="buttons">
                <button className="mini" onClick={() => void copyLogs()} disabled={logs.length === 0}>
                  复制
                </button>
                <button className="mini" onClick={() => setLogs([])} disabled={logs.length === 0}>
                  清空
                </button>
                <button
                  className="mini"
                  onClick={() => void revealLogDir()}
                  disabled={!info || (!info.logDir && !info.configDir)}
                >
                  打开日志目录
                </button>
              </div>
            </div>
            <div className="logbox grow" ref={logBox}>
              {logs.length === 0 ? (
                <div className="muted">暂无日志</div>
              ) : (
                logs.map((line, index) => <div key={index}>{line}</div>)
              )}
            </div>
          </section>
        </main>
      )}

      {tab === "passwords" && (
        <main className="page">
          <section className="panel grow">
            <div className="panel-head">
              <h3>密码库（{passwords.length}）</h3>
              <div className="buttons">
                <button className="mini" onClick={() => void loadPasswords()}>
                  刷新
                </button>
                <button
                  className="mini danger"
                  onClick={() => void clearPasswords()}
                  disabled={passwords.length === 0}
                >
                  清空全部
                </button>
              </div>
            </div>
            <div className="pwadd">
              <input
                type="password"
                autoComplete="off"
                spellCheck={false}
                placeholder="添加密码（明文保存，仅存本机）"
                value={pwInput}
                onChange={(event) => setPwInput(event.target.value)}
                onKeyDown={(event) => {
                  if (event.key === "Enter") void addPassword();
                }}
              />
              <button className="primary" onClick={() => void addPassword()} disabled={pwInput.trim().length === 0}>
                添加密码
              </button>
            </div>
            <div className="pwlist">
              {passwords.length === 0 ? (
                <div className="pwempty">密码库为空 —— 添加后，每个批次会按顺序自动尝试这些密码</div>
              ) : (
                passwords.map((value, index) => {
                  const revealed = pwRevealed.includes(index);
                  return (
                    <div className="pwitem" key={index}>
                      <span className="pwidx">#{index + 1}</span>
                      <span className="pwval" title={revealed ? "点击「隐藏」可重新掩码" : "点击「显示」查看明文"}>
                        {revealed ? value : maskPassword(value)}
                      </span>
                      <span className="pwactions">
                        <button
                          className="mini"
                          onClick={() => togglePwReveal(index)}
                          title={revealed ? "重新掩码" : "显示明文"}
                        >
                          {revealed ? "🙈 隐藏" : "👁 显示"}
                        </button>
                        <button className="mini danger" onClick={() => void removePassword(index)}>
                          删除
                        </button>
                      </span>
                    </div>
                  );
                })
              )}
            </div>
            <div className="pwmeta">
              <div>
                <b>明文存放路径</b>：
                <span className="mono">{configPath || (info ? info.configPath : "") || "—"}</span>
              </div>
              <div>密码以明文保存在本机配置文件中，仅存储于本机、不会上传网络；请避免在共享电脑上使用。</div>
              <div className="muted small">
                批次开始时会自动按顺序尝试密码库中的全部密码；队列页的「本次追加密码」只对本次运行生效。
              </div>
            </div>
          </section>
        </main>
      )}

      {tab === "settings" && (
        <main className="page">
          <SettingsTab
            settings={settings}
            info={info}
            configPath={configPath}
            onSave={saveSettings}
            onThemeChange={changeTheme}
            onCleanupStaging={cleanupStaging}
            onForgetProcessed={forgetProcessed}
            onOpenLogDir={revealLogDir}
            notify={setToast}
            pushLog={pushLog}
          />
        </main>
      )}

      {toast && <div className="toast">{toast}</div>}
    </div>
  );
}

/* ================= 任务行 ================= */

interface TaskRowProps {
  task: Task;
  running: boolean;
  onRetry: (path: string) => void;
  onReextract: (path: string) => void;
  onReveal: (path: string) => void;
  onInspect: (path: string) => void;
  onRemove: (key: string) => void;
  onToggleWarnings: (key: string) => void;
}

function TaskRow({ task, running, onRetry, onReextract, onReveal, onInspect, onRemove, onToggleWarnings }: TaskRowProps) {
  const detection = task.detection ?? null;
  const size = task.size ?? detection?.size ?? null;
  const warnings = task.warnings ?? [];
  const open = task.warningsOpen === true && warnings.length > 0;
  const rowPercent =
    task.percent != null ? task.percent : task.phase === "done" && (task.status === "success" || task.status === "skipped") ? 100 : 0;
  const stateClass =
    task.phase === "running" ? "st-run" : task.phase === "queued" ? "st-queued" : "st-" + (task.status ?? "done");
  const badge =
    task.phase === "queued" ? "待处理" : task.phase === "running" ? stageLabel(task.stage) : statusLabel(task.status);
  const isRetryable =
    task.phase === "done" &&
    (task.status === "failed" || task.status === "needs_password" || task.status === "cancelled");
  const isReextract = task.phase === "done" && (task.status === "success" || task.status === "skipped");
  const notes = detection ? detection.notes.join("；") : "";
  return (
    <Fragment>
      <tr className={stateClass}>
        <td className="name" title={task.path}>
          {task.name}
        </td>
        <td className="kind" title={notes}>
          {detection ? (
            <>
              {kindLabel(detection.kind)}
              {detection.format ? <span className="muted"> · {detection.format}</span> : null}
            </>
          ) : (
            <span className="muted">—</span>
          )}
        </td>
        <td className="size">{size != null ? humanBytes(size) : <span className="muted">—</span>}</td>
        <td className={"pwstate" + (detection?.password_state === "needs_password" ? " needs" : "")}>
          {detection ? passwordStateLabel(detection.password_state) : <span className="muted">—</span>}
        </td>
        <td className="status">
          <span className={"badge " + stateClass}>{badge}</span>
          {task.line ? (
            <span className="linemini muted" title={task.line}>
              {" "}
              {task.line}
            </span>
          ) : null}
        </td>
        <td>
          <div className="progress" title={task.percent != null ? task.percent + "%" : statusLabel(task.status)}>
            <div className="bar" style={{ width: rowPercent + "%" }} />
            <span className="pct">
              {task.percent != null ? task.percent + "%" : rowPercent > 0 ? rowPercent + "%" : ""}
            </span>
          </div>
        </td>
        <td className="result">
          {task.status === "success" && (
            <span>
              {task.files ?? 0} 项 · {humanBytes(task.bytes ?? 0)} · {((task.elapsed_ms ?? 0) / 1000).toFixed(2)}s
            </span>
          )}
          {task.reason ? (
            <span className="muted reason" title={task.reason}>
              {" "}
              {task.reason}
            </span>
          ) : null}
          {task.status === "skipped" && !task.reason ? <span className="muted">已跳过</span> : null}
          {warnings.length > 0 ? (
            <button className="mini warnbtn" onClick={() => onToggleWarnings(task.key)}>
              警告 {warnings.length}
            </button>
          ) : null}
        </td>
        <td className="rowactions">
          {task.phase === "queued" && (
            <>
              <button className="mini" onClick={() => onInspect(task.path)}>
                探测
              </button>
              <button className="mini" disabled={running} onClick={() => onRemove(task.key)}>
                移除
              </button>
            </>
          )}
          {task.phase === "done" && (
            <>
              {isRetryable && (
                <button className="mini primary" disabled={running} onClick={() => onRetry(task.path)}>
                  重试
                </button>
              )}
              {isReextract && (
                <button className="mini" disabled={running} onClick={() => onReextract(task.path)}>
                  重新解压
                </button>
              )}
              {task.dest ? (
                <button className="mini" onClick={() => onReveal(task.dest ?? "")}>
                  定位
                </button>
              ) : null}
              <button className="mini" disabled={running} onClick={() => onRemove(task.key)}>
                移除
              </button>
            </>
          )}
        </td>
      </tr>
      {open && (
        <tr className="warnrow">
          <td colSpan={8}>
            <div className="warnbox">
              <b>警告（{warnings.length}）</b>
              <ul>
                {warnings.map((warning, index) => (
                  <li key={index}>{warning}</li>
                ))}
              </ul>
            </div>
          </td>
        </tr>
      )}
    </Fragment>
  );
}

/* ================= 设置面板 ================= */

interface SettingsTabProps {
  settings: Settings;
  info: AppInfo | null;
  configPath: string;
  onSave: (next: Settings) => Promise<void>;
  onThemeChange: (theme: ThemeMode) => Promise<void>;
  onCleanupStaging: (outputRoot: string | null) => Promise<void>;
  onForgetProcessed: () => Promise<void>;
  onOpenLogDir: () => Promise<void>;
  notify: (message: string) => void;
  pushLog: (line: string) => void;
}

function SettingsTab({
  settings,
  info,
  configPath,
  onSave,
  onThemeChange,
  onCleanupStaging,
  onForgetProcessed,
  onOpenLogDir,
  notify,
  pushLog,
}: SettingsTabProps) {
  const [draft, setDraft] = useState<Settings>(settings);
  const [licenseOpen, setLicenseOpen] = useState(false);
  const [licenseText, setLicenseText] = useState<string | null>(null);
  const [licenseLoading, setLicenseLoading] = useState(false);
  const [licenseError, setLicenseError] = useState<string | null>(null);
  const [diag, setDiag] = useState<Diagnostics | null>(null);
  const [diagLoading, setDiagLoading] = useState(false);
  const [diagError, setDiagError] = useState<string | null>(null);

  useEffect(() => {
    setDraft(settings);
  }, [settings]);

  useEffect(() => {
    if (!licenseOpen) return;
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") setLicenseOpen(false);
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [licenseOpen]);

  useEffect(() => {
    let disposed = false;
    setDiagLoading(true);
    void (async () => {
      try {
        const data = await invoke<Diagnostics>("diagnostics");
        if (!disposed) {
          setDiag(data ?? null);
          setDiagError(null);
        }
      } catch (error) {
        if (!disposed) setDiagError("诊断命令不可用: " + errText(error));
      } finally {
        if (!disposed) setDiagLoading(false);
      }
    })();
    return () => {
      disposed = true;
    };
  }, []);

  async function openLicense() {
    setLicenseOpen(true);
    setLicenseLoading(true);
    setLicenseError(null);
    try {
      const text = await invoke<string>("seven_zip_license");
      setLicenseText(typeof text === "string" ? text : String(text));
    } catch (error) {
      const message = "读取 7-Zip 许可证失败: " + errText(error);
      setLicenseError(message);
      notify(message);
      pushLog(message);
    } finally {
      setLicenseLoading(false);
    }
  }

  function closeLicense() {
    setLicenseOpen(false);
  }

  function localDiagnostics(): Diagnostics {
    return {
      version: info?.version,
      configDir: info?.configDir,
      settingsPath: info?.settingsPath,
      sevenZip: info?.sevenZip ?? null,
      sevenZipVersion: info?.sevenZipVersion ?? null,
      logPath: info?.logPath ?? null,
      logDir: info?.logDir ?? null,
      stagingPrefix: info?.stagingPrefix,
      settingsJson: JSON.stringify(draft, null, 2),
    };
  }

  async function copyDiagnostics() {
    let payload: Diagnostics | null = diag;
    try {
      payload = await invoke<Diagnostics>("diagnostics");
      setDiag(payload);
      setDiagError(null);
    } catch (error) {
      pushLog("diagnostics 命令不可用，改用前端本地信息: " + errText(error));
      payload = payload ?? localDiagnostics();
    }
    try {
      await navigator.clipboard.writeText(JSON.stringify(payload ?? {}, null, 2));
      notify("诊断信息已复制到剪贴板");
    } catch (error) {
      notify("复制诊断信息失败: " + errText(error));
    }
  }

  function patch<K extends keyof Settings>(key: K, value: Settings[K]) {
    setDraft((prev) => ({ ...prev, [key]: value }));
  }

  async function pickOutputRoot() {
    let picked: string | null = null;
    let backendOk = true;
    try {
      picked = await invoke<string | null>("pick_output_dir");
    } catch (error) {
      backendOk = false;
      pushLog("pick_output_dir 不可用，改用前端对话框: " + errText(error));
    }
    if (backendOk) {
      if (picked) patch("outputRoot", picked);
      return;
    }
    try {
      const dir = await openDialog({ multiple: false, directory: true, title: "选择输出根目录" });
      if (typeof dir === "string" && dir.length > 0) patch("outputRoot", dir);
    } catch (error) {
      notify("选择目录失败: " + errText(error));
    }
  }

  function save() {
    const normalized: Settings = {
      ...draft,
      theme: draft.theme === "light" || draft.theme === "dark" ? draft.theme : "system",
      outputRoot: draft.outputRoot && draft.outputRoot.trim().length > 0 ? draft.outputRoot.trim() : null,
      jobs: clampInt(draft.jobs, 1, 8),
      maxDepth: clampInt(draft.maxDepth, 1, 8),
      minFreeRatio: clampNum(draft.minFreeRatio, 1, 3, 1.1),
    };
    setDraft(normalized);
    void onSave(normalized);
  }

  return (
    <section className="panel settings">
      <div className="panel-head">
        <h3>设置</h3>
        <div className="buttons">
          <button className="primary" onClick={save}>
            保存设置
          </button>
        </div>
      </div>
      <div className="settingsbody">
        <section className="section">
          <h4>外观</h4>
          <div className="field">
            <label>主题</label>
            <div className="fieldrow">
              <select
                value={draft.theme}
                onChange={(event) => {
                  const theme = event.target.value as ThemeMode;
                  setDraft((prev) => ({ ...prev, theme }));
                  void onThemeChange(theme);
                }}
              >
                <option value="system">跟随系统</option>
                <option value="light">浅色</option>
                <option value="dark">深色</option>
              </select>
              <span className="muted small">切换后立即生效并保存（settings.theme）</span>
            </div>
          </div>
        </section>

        <section className="section">
          <h4>输出</h4>
          <div className="field">
            <label>输出根目录</label>
            <div className="fieldrow">
              <input
                type="text"
                value={draft.outputRoot ?? ""}
                placeholder="留空 = 解压到每个压缩包所在目录"
                onChange={(event) =>
                  patch("outputRoot", event.target.value.trim().length > 0 ? event.target.value : null)
                }
              />
              <button className="mini" onClick={() => void pickOutputRoot()}>
                浏览…
              </button>
              <button className="mini" onClick={() => patch("outputRoot", null)}>
                清除
              </button>
            </div>
          </div>
          <div className="grid2">
            <div className="field">
              <label>目录结构</label>
              <select value={draft.layout} onChange={(event) => patch("layout", event.target.value as LayoutMode)}>
                <option value="smart">smart（智能：单层目录时直接展开）</option>
                <option value="folder">folder（总是建子目录）</option>
                <option value="direct">direct（直接解到根目录）</option>
              </select>
            </div>
            <div className="field">
              <label>冲突处理</label>
              <select
                value={draft.conflict}
                onChange={(event) => patch("conflict", event.target.value as ConflictMode)}
              >
                <option value="rename">rename（自动改名）</option>
                <option value="skip">skip（跳过已存在）</option>
                <option value="overwrite">overwrite（覆盖）</option>
              </select>
            </div>
            <div className="field">
              <label>并发任务数（1–8）</label>
              <input
                type="number"
                min={1}
                max={8}
                value={draft.jobs}
                onChange={(event) => patch("jobs", clampInt(Number(event.target.value), 1, 8))}
              />
            </div>
            <div className="field">
              <label>最大嵌套层数（1–8）</label>
              <input
                type="number"
                min={1}
                max={8}
                value={draft.maxDepth}
                onChange={(event) => patch("maxDepth", clampInt(Number(event.target.value), 1, 8))}
              />
            </div>
            <div className="field">
              <label>最小剩余空间倍率（1–3）</label>
              <input
                type="number"
                min={1}
                max={3}
                step={0.05}
                value={draft.minFreeRatio}
                onChange={(event) => patch("minFreeRatio", clampNum(Number(event.target.value), 1, 3, 1.1))}
              />
            </div>
          </div>
        </section>

        <section className="section">
          <h4>行为</h4>
          <div className="checks">
            <label className="check">
              <input
                type="checkbox"
                checked={draft.skipProcessed}
                onChange={(event) => patch("skipProcessed", event.target.checked)}
              />
              跳过已成功处理过的文件（默认关闭；开启后按 processed.json 指纹跳过）
            </label>
            <label className="check">
              <input
                type="checkbox"
                checked={draft.recursive}
                onChange={(event) => patch("recursive", event.target.checked)}
              />
              递归展开文件夹（把目录内文件全部加入）
            </label>
            <label className="check">
              <input type="checkbox" checked={draft.nested} onChange={(event) => patch("nested", event.target.checked)} />
              级联解压内层压缩包
            </label>
            <label className="check">
              <input
                type="checkbox"
                checked={draft.stopOnError}
                onChange={(event) => patch("stopOnError", event.target.checked)}
              />
              首个失败/缺密码后停止整批（默认继续其余文件）
            </label>
            <label className="check">
              <input
                type="checkbox"
                checked={draft.tryEmptyPassword}
                onChange={(event) => patch("tryEmptyPassword", event.target.checked)}
              />
              最后尝试空密码
            </label>
            <label className="check">
              <input
                type="checkbox"
                checked={draft.forceDocuments}
                onChange={(event) => patch("forceDocuments", event.target.checked)}
              />
              把 Office（docx/xlsx/pptx…）/ JAR 文档当压缩包解开
            </label>
            <label className="check">
              <input
                type="checkbox"
                checked={draft.keepBroken}
                onChange={(event) => patch("keepBroken", event.target.checked)}
              />
              失败时保留已解出的临时目录（便于排查）
            </label>
            <label className="check">
              <input
                type="checkbox"
                checked={draft.stripJunk}
                onChange={(event) => patch("stripJunk", event.target.checked)}
              />
              清理 __MACOSX / .DS_Store 等垃圾文件
            </label>
            <label className="check">
              <input
                type="checkbox"
                checked={draft.allowExecuteSfx}
                onChange={(event) => patch("allowExecuteSfx", event.target.checked)}
              />
              允许执行 Windows 自解压 EXE（默认按数据解析，不执行；仅 Windows）
            </label>
          </div>
        </section>

        <section className="section">
          <h4>维护</h4>
          <div className="buttons">
            <button onClick={() => void onCleanupStaging(draft.outputRoot ?? null)}>清理临时残留</button>
            <button onClick={() => void onForgetProcessed()}>清除处理记录</button>
          </div>
          <div className="muted small">
            清理临时残留：删除输出根目录（未设置时为系统临时目录）下的 .unparanoia-tmp-* 残留；清除处理记录：删除
            processed.json，之后再次解压不会跳过已成功项。
          </div>
        </section>

        <section className="section">
          <h4>诊断信息</h4>
          <div className="techinfo">
            <div>
              <b>版本</b>：{diag?.version ?? (info ? info.version : "—")}
            </div>
            <div>
              <b>系统</b>：
              {diag && (diag.os || diag.arch) ? (diag.os ?? "?") + " / " + (diag.arch ?? "?") : "—"}
            </div>
            <div>
              <b>7-Zip</b>：{diag?.sevenZipVersion ?? (info ? info.sevenZipVersion ?? "未找到" : "—")}
              {diag?.sevenZip ?? info?.sevenZip ? (
                <span className="mono muted"> ({diag?.sevenZip ?? info?.sevenZip})</span>
              ) : null}
            </div>
            <div>
              <b>配置目录</b>：<span className="mono">{diag?.configDir ?? (info ? info.configDir : "—")}</span>
            </div>
            <div>
              <b>日志路径</b>：<span className="mono">{diag?.logPath ?? info?.logPath ?? "—"}</span>
            </div>
            {diag?.processedCount != null ? (
              <div>
                <b>处理记录</b>：{diag.processedCount} 条
                {diag.processedPath ? <span className="mono muted"> ({diag.processedPath})</span> : null}
              </div>
            ) : null}
            {diag?.passwordCount != null ? (
              <div>
                <b>密码库</b>：{diag.passwordCount} 条（明文）
              </div>
            ) : null}
            {diagLoading ? <div className="muted small">正在读取诊断信息…</div> : null}
            {diagError ? <div className="bad small">{diagError}</div> : null}
            <div className="muted small">
              日志文件用于排查解压问题；如遇失败，可复制诊断信息或打开日志目录后附上 unparanoia.log。
            </div>
          </div>
          <div className="buttons">
            <button onClick={() => void copyDiagnostics()}>复制诊断信息</button>
            <button onClick={() => void onOpenLogDir()}>打开日志目录</button>
            <button onClick={() => void onForgetProcessed()}>清除处理记录</button>
          </div>
        </section>

        <section className="section">
          <h4>关于 / 7-Zip 许可证</h4>
          <div className="techinfo">
            <div>
              <b>7-Zip</b>：
              {info && info.sevenZipVersion
                ? "内嵌 7-Zip " + info.sevenZipVersion + "（GNU LGPL 2.1 + unRAR 限制），许可证随程序分发"
                : "内嵌 7-Zip 25.01（GNU LGPL 2.1 + unRAR 限制），许可证随程序分发"}
            </div>
            {info && info.sevenZip ? (
              <div>
                <b>位置</b>：<span className="mono">{info.sevenZip}</span>
              </div>
            ) : null}
            <div className="muted small">
              7-Zip 以独立开源组件随程序分发；点击下方按钮可在程序内查看完整许可证全文（含 unRAR 限制条款）。
            </div>
          </div>
          <div className="buttons">
            <button onClick={() => void openLicense()}>查看 7-Zip 许可证</button>
          </div>
        </section>

        <section className="section">
          <h4>运行信息</h4>
          <div className="techinfo">
            <div>
              <b>版本</b>：{info ? info.version : "—"}
            </div>
            <div>
              <b>7-Zip</b>：{info ? info.sevenZipVersion ?? "未找到" : "—"}
              {info && info.sevenZip ? <span className="mono muted"> ({info.sevenZip})</span> : null}
            </div>
            <div>
              <b>配置目录</b>：<span className="mono">{info ? info.configDir : "—"}</span>
            </div>
            <div>
              <b>配置文件</b>：<span className="mono">{configPath || (info ? info.configPath : "") || "—"}</span>
            </div>
            <div>
              <b>设置文件</b>：<span className="mono">{info ? info.settingsPath : "—"}</span>
            </div>
            <div>
              <b>临时目录前缀</b>：<span className="mono">{info ? info.stagingPrefix : "—"}</span>
            </div>
            <div className="muted small">
              密码库以明文保存在配置文件中（见「密码」页），仅存本机；请避免在共享电脑上使用该功能。
            </div>
          </div>
        </section>
      </div>

      {licenseOpen && (
        <div className="licenseoverlay" role="presentation" onClick={closeLicense}>
          <div
            className="licensebox"
            role="dialog"
            aria-modal="true"
            aria-label="7-Zip 许可证"
            onClick={(event) => event.stopPropagation()}
          >
            <div className="licensehead">
              <h4>7-Zip 许可证（GNU LGPL 2.1 + unRAR 限制）</h4>
              <button className="mini" onClick={closeLicense}>
                关闭
              </button>
            </div>
            <div className="licensebody">
              {licenseLoading ? (
                <div className="muted">正在读取许可证…</div>
              ) : licenseError ? (
                <div className="bad">{licenseError}</div>
              ) : (
                <pre className="licensepre">{licenseText ?? ""}</pre>
              )}
            </div>
          </div>
        </div>
      )}
    </section>
  );
}
