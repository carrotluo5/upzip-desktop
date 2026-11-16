import { DEFAULT_SETTINGS, FileKind, PasswordState, Settings, TaskStatus, ThemeMode } from "./types";

export const CSS_MASK_SUPPORTED = (() => {
  try {
    return (
      typeof CSS !== "undefined" &&
      typeof CSS.supports === "function" &&
      CSS.supports("-webkit-text-security", "disc")
    );
  } catch {
    return false;
  }
})();

export const BACKSLASH = String.fromCharCode(92);

export function basename(p: string): string {
  const idx = Math.max(p.lastIndexOf("/"), p.lastIndexOf(BACKSLASH));
  return idx >= 0 ? p.slice(idx + 1) : p;
}

export function humanBytes(n: number): string {
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

export function kindLabel(kind: FileKind): string {
  switch (kind) {
    case "archive":
      return "压缩包";
    case "masked_archive":
      return "伪装压缩包";
    case "ssefe_encrypted":
      return ".enc 容器";
    case "sfx_executable":
      return "自解压 EXE";
    case "not_archive":
      return "非压缩文件";
    default:
      return "未知";
  }
}

export function passwordStateLabel(state: PasswordState): string {
  switch (state) {
    case "no_password":
      return "无密码";
    case "needs_password":
      return "需密码";
    default:
      return "未知";
  }
}

export function statusLabel(status: TaskStatus | null | undefined): string {
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

export function stageLabel(stage: string | null | undefined): string {
  switch (stage) {
    case "detect":
      return "识别";
    case "decrypt":
      return "解封容器";
    case "extract":
      return "解压";
    case "nested":
      return "级联内层";
    case "land":
      return "写出";
    case "password":
      return "匹配密码";
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

export function errText(error: unknown): string {
  if (error instanceof Error) return error.message;
  if (typeof error === "string") return error;
  try {
    return JSON.stringify(error);
  } catch {
    return String(error);
  }
}

export function toPath(value: unknown): string | null {
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

export function asPathList(value: unknown): string[] {
  if (!Array.isArray(value)) return [];
  const out: string[] = [];
  for (const item of value) {
    const path = toPath(item);
    if (path) out.push(path);
  }
  return out;
}

export function clampInt(value: number, min: number, max: number): number {
  if (!Number.isFinite(value)) return min;
  return Math.min(max, Math.max(min, Math.round(value)));
}

export function clampNum(value: number, min: number, max: number, fallback: number): number {
  if (!Number.isFinite(value)) return fallback;
  return Math.min(max, Math.max(min, value));
}

const THEME_CACHE_KEY = "upzip.theme";

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

export function applyTheme(theme: ThemeMode): "light" | "dark" {
  const resolved: "light" | "dark" = theme === "system" ? systemTheme() : theme;
  if (typeof document !== "undefined") {
    document.documentElement.dataset.theme = resolved;
  }
  try {
    window.localStorage.setItem(THEME_CACHE_KEY, resolved);
  } catch {
    /* ignore */
  }
  return resolved;
}

export function bootstrapTheme(): void {
  if (typeof document === "undefined") return;
  applyTheme(cachedTheme() ?? "system");
}

export function themeLabel(theme: ThemeMode): string {
  switch (theme) {
    case "light":
      return "浅色";
    case "dark":
      return "深色";
    default:
      return "跟随系统";
  }
}

export function normalizeSettings(raw: Partial<Settings> | null | undefined): Settings {
  const merged: Settings = { ...DEFAULT_SETTINGS, ...(raw ?? {}) };
  const theme = merged.theme;
  return {
    ...merged,
    theme: theme === "light" || theme === "dark" || theme === "system" ? theme : "system",
  };
}

export function maskPassword(value: string): string {
  const count = Math.min(Math.max(value.length, 4), 12);
  return "•".repeat(count);
}
