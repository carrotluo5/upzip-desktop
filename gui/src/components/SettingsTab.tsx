import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { AppInfo, ConflictMode, Diagnostics, LayoutMode, Settings, ThemeMode } from "../types";
import { clampInt, clampNum, errText } from "../utils";

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

interface ToggleRowProps {
  label: string;
  hint?: string;
  checked: boolean;
  onChange: (checked: boolean) => void;
}

function ToggleRow({ label, hint, checked, onChange }: ToggleRowProps) {
  return (
    <label className="toggle-row">
      <div className="toggle-text">
        <span className="toggle-label">{label}</span>
        {hint ? <span className="toggle-hint">{hint}</span> : null}
      </div>
      <span className={"switch" + (checked ? " on" : "")}>
        <input type="checkbox" checked={checked} onChange={(e) => onChange(e.target.checked)} />
        <span className="thumb" />
      </span>
    </label>
  );
}

export function SettingsTab({
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
    void (async () => {
      try {
        const data = await invoke<Diagnostics>("diagnostics");
        if (!disposed) setDiag(data ?? null);
      } catch {
        /* fallback to AppInfo */
      }
    })();
    return () => {
      disposed = true;
    };
  }, []);

  function patch<K extends keyof Settings>(key: K, value: Settings[K]) {
    setDraft((prev) => ({ ...prev, [key]: value }));
  }

  async function pickOutputRoot() {
    try {
      const picked = await invoke<string | null>("pick_output_dir");
      if (picked) patch("outputRoot", picked);
      return;
    } catch {
      /* fallback */
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

  async function copyDiagnostics() {
    let payload: Diagnostics | null = diag;
    try {
      payload = await invoke<Diagnostics>("diagnostics");
      setDiag(payload);
    } catch {
      payload = payload ?? {
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
    try {
      await navigator.clipboard.writeText(JSON.stringify(payload ?? {}, null, 2));
      notify("诊断信息已复制到剪贴板");
    } catch (error) {
      notify("复制诊断信息失败: " + errText(error));
    }
  }

  return (
    <div className="page settings-page">
      <div className="settings-topbar">
        <div>
          <h3>设置</h3>
          <p className="muted small">调整主题、输出规则、解包行为与系统维护</p>
        </div>
        <button className="btn-primary" onClick={save}>
          保存设置
        </button>
      </div>

      <div className="settings-scroll">
        {/* 外观 */}
        <section className="card">
          <h4 className="section-title">外观</h4>
          <div className="setting-row">
            <div>
              <div className="setting-label">界面主题</div>
              <div className="muted small">切换后立即生效并保存</div>
            </div>
            <div className="segmented">
              {(["system", "light", "dark"] as ThemeMode[]).map((mode) => (
                <button
                  key={mode}
                  type="button"
                  className={draft.theme === mode ? "active" : ""}
                  onClick={() => {
                    patch("theme", mode);
                    void onThemeChange(mode);
                  }}
                >
                  {mode === "system" ? "跟随系统" : mode === "light" ? "浅色" : "深色"}
                </button>
              ))}
            </div>
          </div>
        </section>

        {/* 输出与性能 */}
        <section className="card">
          <h4 className="section-title">输出与性能</h4>
          <div className="field-block">
            <label className="setting-label">输出根目录</label>
            <div className="field-inline">
              <input
                type="text"
                className="input-clean"
                value={draft.outputRoot ?? ""}
                placeholder="留空 = 解压到原压缩包所在目录"
                onChange={(e) => patch("outputRoot", e.target.value.trim().length > 0 ? e.target.value : null)}
              />
              <button className="btn-soft" onClick={() => void pickOutputRoot()}>
                浏览…
              </button>
              <button className="btn-ghost" onClick={() => patch("outputRoot", null)}>
                清除
              </button>
            </div>
          </div>

          <div className="grid2">
            <div className="field-block">
              <label className="setting-label">目录结构</label>
              <select
                className="select-clean"
                value={draft.layout}
                onChange={(e) => patch("layout", e.target.value as LayoutMode)}
              >
                <option value="smart">smart（智能：单条目直接展开）</option>
                <option value="folder">folder（总是新建子文件夹）</option>
                <option value="direct">direct（直接解到根目录）</option>
              </select>
            </div>
            <div className="field-block">
              <label className="setting-label">文件冲突</label>
              <select
                className="select-clean"
                value={draft.conflict}
                onChange={(e) => patch("conflict", e.target.value as ConflictMode)}
              >
                <option value="rename">rename（自动重命名）</option>
                <option value="skip">skip（跳过已存在）</option>
                <option value="overwrite">overwrite（直接覆盖）</option>
              </select>
            </div>
            <div className="field-block">
              <label className="setting-label">并发任务数（1–8）</label>
              <input
                type="number"
                className="input-clean"
                min={1}
                max={8}
                value={draft.jobs}
                onChange={(e) => patch("jobs", clampInt(Number(e.target.value), 1, 8))}
              />
            </div>
            <div className="field-block">
              <label className="setting-label">最大嵌套层数（1–8）</label>
              <input
                type="number"
                className="input-clean"
                min={1}
                max={8}
                value={draft.maxDepth}
                onChange={(e) => patch("maxDepth", clampInt(Number(e.target.value), 1, 8))}
              />
            </div>
            <div className="field-block">
              <label className="setting-label">最小剩余空间倍率（1–3）</label>
              <input
                type="number"
                className="input-clean"
                min={1}
                max={3}
                step={0.05}
                value={draft.minFreeRatio}
                onChange={(e) => patch("minFreeRatio", clampNum(Number(e.target.value), 1, 3, 1.1))}
              />
            </div>
          </div>
        </section>

        {/* 解包行为 */}
        <section className="card">
          <h4 className="section-title">解包行为</h4>
          <div className="toggle-list">
            <ToggleRow
              label="级联解压内层压缩包"
              hint="自动递归展开嵌套在压缩包内部的归档文件"
              checked={draft.nested}
              onChange={(v) => patch("nested", v)}
            />
            <ToggleRow
              label="递归展开文件夹"
              hint="拖入或选择文件夹时扫描其全部子目录"
              checked={draft.recursive}
              onChange={(v) => patch("recursive", v)}
            />
            <ToggleRow
              label="清理系统垃圾条目"
              hint="自动移除 __MACOSX、.DS_Store、._*、Thumbs.db 等"
              checked={draft.stripJunk}
              onChange={(v) => patch("stripJunk", v)}
            />
            <ToggleRow
              label="最后尝试空密码"
              hint="候选密码均未命中时尝试空密码"
              checked={draft.tryEmptyPassword}
              onChange={(v) => patch("tryEmptyPassword", v)}
            />
            <ToggleRow
              label="跳过已成功处理的文件"
              hint="默认关闭；开启后按 processed.json 指纹跳过重复文件"
              checked={draft.skipProcessed}
              onChange={(v) => patch("skipProcessed", v)}
            />
            <ToggleRow
              label="遇错立即停止整批"
              hint="首个任务失败或缺密码后停止派发后续任务"
              checked={draft.stopOnError}
              onChange={(v) => patch("stopOnError", v)}
            />
            <ToggleRow
              label="展开 Office / JAR 文档容器"
              hint="把 docx / xlsx / pptx / jar / apk 等当作压缩包解开"
              checked={draft.forceDocuments}
              onChange={(v) => patch("forceDocuments", v)}
            />
            <ToggleRow
              label="失败时保留临时目录"
              hint="便于排查损坏压缩包的残留内容"
              checked={draft.keepBroken}
              onChange={(v) => patch("keepBroken", v)}
            />
            <ToggleRow
              label="允许执行 Windows 自解压 EXE"
              hint="仅 Windows 有效；默认仅按数据解析，不运行可执行文件"
              checked={draft.allowExecuteSfx}
              onChange={(v) => patch("allowExecuteSfx", v)}
            />
          </div>
        </section>

        {/* 维护与诊断（去重整合） */}
        <section className="card">
          <h4 className="section-title">维护与诊断</h4>
          <div className="btn-group wrap">
            <button className="btn-soft" onClick={() => void onCleanupStaging(draft.outputRoot ?? null)}>
              清理临时残留
            </button>
            <button className="btn-soft" onClick={() => void onForgetProcessed()}>
              清除处理记录
            </button>
            <button className="btn-soft" onClick={() => void copyDiagnostics()}>
              复制诊断 JSON
            </button>
            <button className="btn-soft" onClick={() => void onOpenLogDir()}>
              打开日志目录
            </button>
            <button className="btn-soft" onClick={() => void openLicense()}>
              7-Zip 许可证
            </button>
          </div>

          <div className="kv-grid">
            <div className="kv-item">
              <span className="kv-k">版本 / 平台</span>
              <span className="kv-v mono">
                v{diag?.version ?? info?.version ?? "—"}
                {diag && (diag.os || diag.arch) ? ` · ${diag.os ?? "?"}/${diag.arch ?? "?"}` : ""}
              </span>
            </div>
            <div className="kv-item">
              <span className="kv-k">7-Zip 内核</span>
              <span className="kv-v mono" title={diag?.sevenZip ?? info?.sevenZip ?? ""}>
                {diag?.sevenZipVersion ?? info?.sevenZipVersion ?? "未找到"}
              </span>
            </div>
            <div className="kv-item">
              <span className="kv-k">配置目录</span>
              <span className="kv-v mono">{diag?.configDir ?? info?.configDir ?? "—"}</span>
            </div>
            <div className="kv-item">
              <span className="kv-k">配置文件</span>
              <span className="kv-v mono">{configPath || info?.configPath || "—"}</span>
            </div>
            <div className="kv-item">
              <span className="kv-k">日志文件</span>
              <span className="kv-v mono">{diag?.logPath ?? info?.logPath ?? "—"}</span>
            </div>
            <div className="kv-item">
              <span className="kv-k">处理记录</span>
              <span className="kv-v mono">
                {diag?.processedCount != null ? `${diag.processedCount} 条` : "—"}
              </span>
            </div>
          </div>
        </section>
      </div>

      {licenseOpen && (
        <div className="modal-overlay" role="presentation" onClick={() => setLicenseOpen(false)}>
          <div
            className="modal-box"
            role="dialog"
            aria-modal="true"
            aria-label="7-Zip 许可证"
            onClick={(e) => e.stopPropagation()}
          >
            <div className="modal-head">
              <h4>7-Zip 许可证（GNU LGPL 2.1 + unRAR 限制）</h4>
              <button className="btn-ghost" onClick={() => setLicenseOpen(false)}>
                关闭
              </button>
            </div>
            <div className="modal-body">
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
    </div>
  );
}
