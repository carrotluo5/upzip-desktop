import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { confirm as confirmDialog, open as openDialog } from "@tauri-apps/plugin-dialog";
import {
  AppInfo,
  BatchState,
  DEFAULT_SETTINGS,
  Detection,
  EMPTY_COUNTS,
  LiveCounts,
  REQUEUE,
  Settings,
  StartRequest,
  Summary,
  Tab,
  Task,
  TaskEvent,
  ThemeMode,
} from "./types";
import {
  applyTheme,
  asPathList,
  BACKSLASH,
  basename,
  bootstrapTheme,
  errText,
  humanBytes,
  kindLabel,
  normalizeSettings,
  passwordStateLabel,
  stageLabel,
  statusLabel,
  themeLabel,
} from "./utils";
import {
  FINISHED_TASK_RESET,
  displayVersion,
  finishedPercent,
  isPasswordEmpty,
  passwordToSubmit,
} from "./task-logic";
import { KeyIcon, LogIcon, QueueIcon, SettingsIcon } from "./components/Icons";
import { QueueTab } from "./components/QueueTab";
import { PasswordsTab } from "./components/PasswordsTab";
import { LogsTab } from "./components/LogsTab";
import { SettingsTab } from "./components/SettingsTab";
import "./App.css";

export { bootstrapTheme };

export default function App() {
  const [info, setInfo] = useState<AppInfo | null>(null);
  const [settings, setSettings] = useState<Settings>(DEFAULT_SETTINGS);
  const [configPath, setConfigPath] = useState("");
  const [password, setPassword] = useState("");
  const [rememberPassword, setRememberPassword] = useState(false);
  const [passwords, setPasswords] = useState<string[]>([]);
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

  const idKey = useRef<Map<number, string>>(new Map());
  const tasksRef = useRef<Task[]>([]);
  const batchKeysRef = useRef<string[]>([]);

  useEffect(() => {
    tasksRef.current = tasks;
  }, [tasks]);

  // The backend already writes every TaskEvent into upzip.log, so lines that
  // only mirror backend events pass `persist = false` instead of duplicating
  // them on disk.
  const pushLog = useCallback((line: string, persist = true) => {
    const stamp = new Date().toLocaleTimeString("zh-CN", { hour12: false });
    setLogs((prev) => {
      const next = prev.concat("[" + stamp + "] " + line);
      return next.length > 2000 ? next.slice(next.length - 2000) : next;
    });
    if (persist) void invoke("append_log", { line }).catch(() => {});
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
          setToast("没有新增文件（均已在队列中）");
          return;
        }
        setTab("queue");
        setToast(
          "已加入 " + result.added + " 个文件" + (result.requeued > 0 ? "，重新排队 " + result.requeued + " 个" : ""),
        );
        pushLog(
          source +
            "：新增 " +
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

  const addPassword = useCallback(
    async (raw: string): Promise<boolean> => {
      // Passwords may legitimately start or end with spaces: reject only the
      // empty string, matching upzip-core / upzip-cli.
      if (isPasswordEmpty(raw)) {
        setToast("密码不能为空");
        return false;
      }
      try {
        const added = await invoke<boolean>("add_password", { password: raw });
        if (added) {
          pushLog("已添加密码到密码库（明文保存）");
          setToast("已添加到密码库");
        } else {
          setToast("该密码已在密码库中");
        }
        await loadPasswords();
        return true;
      } catch (error) {
        setToast("添加密码失败: " + errText(error));
        return false;
      }
    },
    [loadPasswords, pushLog],
  );

  const removePassword = useCallback(
    async (index: number) => {
      try {
        const removed = await invoke<boolean>("remove_password", { index });
        if (removed) {
          pushLog("已删除密码库第 " + (index + 1) + " 条");
          setToast("已删除该密码");
        } else {
          setToast("密码不存在，密码库可能已变化");
        }
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
      await loadPasswords();
      pushLog("已清空密码库");
      setToast("已清空密码库");
    } catch (error) {
      setToast("清空密码库失败: " + errText(error));
    }
  }, [passwords.length, loadPasswords, pushLog]);

  /* ---------- 主题 ---------- */

  const changeTheme = useCallback(
    async (theme: ThemeMode) => {
      const next: Settings = { ...settings, theme };
      setSettings(next);
      applyTheme(theme);
      try {
        await invoke("settings_set", { settings: next });
        pushLog("外观主题已切换为" + themeLabel(theme));
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
        setToast("请先添加文件或文件夹");
        return;
      }
      if (running) {
        setToast("已有批次正在运行");
        return;
      }
      const effective: Settings = { ...settings, ...(override ?? {}) };
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
        password: passwordToSubmit(password),
        rememberPassword,
        settings: effective,
      };
      setTab("queue");
      pushLog("开始批次：共 " + files.length + " 个文件");
      try {
        await invoke("start_batch", { req });
        if (rememberPassword && !isPasswordEmpty(password)) {
          setPassword("");
          setRememberPassword(false);
          pushLog("本次密码已保存到密码库");
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

  const reextractOne = useCallback(
    (path: string) => {
      retryOne(path, { skipProcessed: false });
    },
    [retryOne],
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

  const toggleWarnings = useCallback((key: string) => {
    setTasks((prev) => prev.map((t) => (t.key === key ? { ...t, warningsOpen: !t.warningsOpen } : t)));
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
        setToast(
          basename(path) + "：" + kindLabel(detection.kind) + " · " + passwordStateLabel(detection.password_state),
        );
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
        pushLog("清理临时残留：删除 " + deleted + " 个残留");
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
      "清除处理记录？删除 processed.json 后，已成功解压过的文件不会再被跳过。",
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
      await navigator.clipboard.writeText(logs.join("\n"));
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
      setToast("日志目录未知");
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
          } catch {
            const picked = await openDialog({ multiple: true, directory: false, title: "选择要解压的文件" });
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
    [addRaw],
  );

  /* ---------- 启动加载 ---------- */

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
      } catch {
        /* ignore */
      }
      try {
        const startup = asPathList(await invoke<unknown>("startup_paths"));
        if (!disposed && startup.length > 0) {
          await addRaw(startup, "启动参数");
        }
      } catch {
        /* ignore */
      }
      try {
        const state = await invoke<BatchState>("batch_state");
        if (!disposed) {
          if (state.running) {
            setRunning(true);
            setPaused(state.paused);
          }
          if (state.summary) setSummary(state.summary);
        }
      } catch {
        /* ignore */
      }
    })();
    return () => {
      disposed = true;
    };
  }, [addRaw, pushLog]);

  /* ---------- 主题监听 ---------- */

  useEffect(() => {
    applyTheme(settings.theme);
    if (settings.theme !== "system" || typeof window.matchMedia !== "function") return;
    const media = window.matchMedia("(prefers-color-scheme: dark)");
    const onChange = () => applyTheme("system");
    media.addEventListener("change", onChange);
    return () => media.removeEventListener("change", onChange);
  }, [settings.theme]);

  /* ---------- 后端事件 ---------- */

  useEffect(() => {
    let disposed = false;
    const unsubs: Array<() => void> = [];

    (async () => {
      const unlistenEvent = await listen<TaskEvent>("upzip://event", (event) => {
        const ev = event.payload;
        // Backend events are already persisted by run_batch_thread; only show
        // them in the UI log.
        const logEvent = (line: string) => pushLog(line, false);
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
            logEvent(
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
            logEvent("#" + ev.id + " 尝试密码（" + ev.source + "）：" + (ev.ok ? "通过" : "失败") + "，第 " + ev.tried + " 次");
            break;
          }
          case "task_password_missing": {
            logEvent("#" + ev.id + " 没有可用密码（已尝试 " + ev.tried + " 个）");
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
              percent: finishedPercent(outcome.status),
              // Finished tasks must not keep showing stale progress text.
              ...FINISHED_TASK_RESET,
            };
            if (outcome.detection) patch.detection = outcome.detection;
            patchById(ev.id, patch);
            const related = outcome.related_paths ?? [];
            if (related.length > 0) {
              const mergedReason = "已并入 " + basename(outcome.path) + "（分卷）";
              const relatedPatch: Partial<Task> = {
                phase: "done",
                status: outcome.status,
                percent: finishedPercent(outcome.status),
                reason: mergedReason,
                ...FINISHED_TASK_RESET,
              };
              const knownKeys = new Set(tasksRef.current.map((t) => t.key));
              for (const rel of related) {
                if (!rel || rel === outcome.path) continue;
                if (!knownKeys.has(rel)) continue;
                patchByKey(rel, relatedPatch);
              }
            }
            if (outcome.status === "failed" || outcome.status === "needs_password") {
              logEvent(basename(outcome.path) + " " + statusLabel(outcome.status) + "：" + (outcome.reason ?? "无详情"));
            }
            break;
          }
          case "log": {
            logEvent(ev.line);
            break;
          }
        }
      });
      if (disposed) {
        unlistenEvent();
        return;
      }
      unsubs.push(unlistenEvent);

      const unlistenDone = await listen<Summary>("upzip://done", (event) => {
        const done = event.payload;
        const batch = batchKeysRef.current;
        const settleKeys =
          batch.length > 0
            ? batch
            : tasksRef.current.filter((t) => t.phase === "queued" || t.phase === "running").map((t) => t.key);
        setSummary(done);
        setRunning(false);
        setPaused(false);
        setActivity("");
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

  /* ---------- Toast & 轮询兜底 ---------- */

  useEffect(() => {
    if (!toast) return;
    const timer = window.setTimeout(() => setToast(null), 3200);
    return () => window.clearTimeout(timer);
  }, [toast]);

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
          void refreshInfo();
        }
      } catch {
        /* ignore */
      }
    };
    const timer = window.setInterval(() => void reconcile(), 2000);
    const onFocus = () => void reconcile();
    window.addEventListener("focus", onFocus);
    return () => {
      cancelled = true;
      window.clearInterval(timer);
      window.removeEventListener("focus", onFocus);
    };
  }, [running, refreshInfo]);

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

  const recentLog = logs.length > 0 ? logs[logs.length - 1] : "";
  const canOpenLogDir = Boolean(info && (info.logDir || info.configDir));

  return (
    <div className={"shell" + (dragging ? " dragging" : "")}>
      <aside className="sidebar">
        <div className="brand">
          {iconOk ? (
            <img
              className="appicon"
              src="/icon.png"
              alt="Upzip"
              width={32}
              height={32}
              onError={() => setIconOk(false)}
            />
          ) : (
            <span className="logo-fallback">U</span>
          )}
          <div className="brand-text">
            <div className="brand-name">Upzip</div>
            <div className="brand-ver">v{displayVersion(info, __APP_VERSION__)}</div>
          </div>
        </div>

        <nav className="nav">
          <button className={"nav-item" + (tab === "queue" ? " active" : "")} onClick={() => setTab("queue")}>
            <QueueIcon />
            <span>队列</span>
            {tasks.length > 0 && <span className="nav-badge">{tasks.length}</span>}
          </button>
          <button className={"nav-item" + (tab === "passwords" ? " active" : "")} onClick={() => setTab("passwords")}>
            <KeyIcon />
            <span>密码库</span>
            {passwords.length > 0 && <span className="nav-badge">{passwords.length}</span>}
          </button>
          <button className={"nav-item" + (tab === "logs" ? " active" : "")} onClick={() => setTab("logs")}>
            <LogIcon />
            <span>日志</span>
          </button>
          <button className={"nav-item" + (tab === "settings" ? " active" : "")} onClick={() => setTab("settings")}>
            <SettingsIcon />
            <span>设置</span>
          </button>
        </nav>

        <div className="sidebar-foot">
          <div className="engine-pill" title={info?.sevenZip ?? "未找到 7-Zip"}>
            <span className={"status-dot " + (info?.sevenZip ? "ok" : "bad")} />
            <span className="mono-ellipsis">
              {info ? (info.sevenZip ? "7-Zip 就绪" : "未找到 7-Zip") : "检测中…"}
            </span>
          </div>
        </div>
      </aside>

      <main className="main-area">
        {tab === "queue" && (
          <QueueTab
            tasks={tasks}
            queuedTasks={queuedTasks}
            retryableTasks={retryableTasks}
            running={running}
            paused={paused}
            dragging={dragging}
            password={password}
            rememberPassword={rememberPassword}
            passwordCount={passwords.length}
            settings={settings}
            summary={summary}
            counts={counts}
            batchProgress={batchProgress}
            activity={activity}
            skipNotice={skipNotice}
            recentLog={recentLog}
            onPasswordChange={setPassword}
            onRememberPasswordChange={setRememberPassword}
            onOpenPasswords={() => setTab("passwords")}
            onPickInput={(mode) => void pickInput(mode)}
            onStart={start}
            onPause={() => void pauseBatch()}
            onResume={() => void resumeBatch()}
            onCancel={() => void cancelBatch()}
            onRetryFailed={retryFailed}
            onClearList={() => void clearList()}
            onForgetProcessed={forgetProcessed}
            onDismissSkipNotice={() => setSkipNotice(null)}
            onReextractAllSkipped={(keys) => void startPaths(keys, { skipProcessed: false })}
            onRetryOne={retryOne}
            onReextractOne={reextractOne}
            onReveal={(p) => void reveal(p)}
            onInspect={(p) => void inspectFile(p)}
            onRemoveTask={removeTask}
            onToggleWarnings={toggleWarnings}
          />
        )}

        {tab === "passwords" && (
          <PasswordsTab
            passwords={passwords}
            configPath={configPath || (info ? info.configPath : "")}
            onRefresh={loadPasswords}
            onAdd={addPassword}
            onRemove={removePassword}
            onClear={clearPasswords}
          />
        )}

        {tab === "logs" && (
          <LogsTab
            logs={logs}
            canOpenDir={canOpenLogDir}
            onCopy={() => void copyLogs()}
            onClear={() => setLogs([])}
            onOpenDir={() => void revealLogDir()}
          />
        )}

        {tab === "settings" && (
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
        )}
      </main>

      {toast && <div className="toast">{toast}</div>}
    </div>
  );
}
