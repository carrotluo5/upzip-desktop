import { useState } from "react";
import { LiveCounts, Settings, Summary, Task } from "../types";
import { CSS_MASK_SUPPORTED, humanBytes } from "../utils";
import { EyeIcon, UploadIcon } from "./Icons";
import { TaskRow } from "./TaskRow";

interface QueueTabProps {
  tasks: Task[];
  queuedTasks: Task[];
  retryableTasks: Task[];
  running: boolean;
  paused: boolean;
  dragging: boolean;
  password: string;
  rememberPassword: boolean;
  passwordCount: number;
  settings: Settings;
  summary: Summary | null;
  counts: LiveCounts;
  batchProgress: { percent: number; known: boolean };
  activity: string;
  skipNotice: string[] | null;
  recentLog: string;
  onPasswordChange: (value: string) => void;
  onRememberPasswordChange: (checked: boolean) => void;
  onOpenPasswords: () => void;
  onPickInput: (mode: "files" | "folder") => void;
  onStart: () => void;
  onPause: () => void;
  onResume: () => void;
  onCancel: () => void;
  onRetryFailed: () => void;
  onClearList: () => void;
  onForgetProcessed: () => Promise<void>;
  onDismissSkipNotice: () => void;
  onReextractAllSkipped: (keys: string[]) => void;
  onRetryOne: (path: string) => void;
  onReextractOne: (path: string) => void;
  onReveal: (path: string) => void;
  onInspect: (path: string) => void;
  onRemoveTask: (key: string) => void;
  onToggleWarnings: (key: string) => void;
}

export function QueueTab({
  tasks,
  queuedTasks,
  retryableTasks,
  running,
  paused,
  dragging,
  password,
  rememberPassword,
  passwordCount,
  settings,
  summary,
  counts,
  batchProgress,
  activity,
  skipNotice,
  recentLog,
  onPasswordChange,
  onRememberPasswordChange,
  onOpenPasswords,
  onPickInput,
  onStart,
  onPause,
  onResume,
  onCancel,
  onRetryFailed,
  onClearList,
  onForgetProcessed,
  onDismissSkipNotice,
  onReextractAllSkipped,
  onRetryOne,
  onReextractOne,
  onReveal,
  onInspect,
  onRemoveTask,
  onToggleWarnings,
}: QueueTabProps) {
  const [showPw, setShowPw] = useState(false);
  const queuedSize = queuedTasks.reduce((acc, t) => acc + (t.size ?? 0), 0);

  return (
    <div className="page">
      {/* 精简拖放区 */}
      <section className={"dropzone" + (dragging ? " hot" : "") + (tasks.length > 0 ? " slim" : "")}>
        <div className="dz-left">
          <div className="dz-icon">
            <UploadIcon />
          </div>
          <div>
            <div className="dz-title">{dragging ? "松开以加入解压队列" : "拖放压缩包或文件夹到这里"}</div>
            <div className="dz-sub">支持 zip / 7z / rar / 分卷、伪装容器（如 .mp4）、自解压 EXE 与 .enc 容器</div>
          </div>
        </div>
        <div className="dz-actions">
          <button className="btn-soft" onClick={() => onPickInput("files")} disabled={running}>
            选择文件
          </button>
          <button className="btn-soft" onClick={() => onPickInput("folder")} disabled={running}>
            选择文件夹
          </button>
        </div>
      </section>

      {/* 跳过提示条 */}
      {skipNotice && (
        <div className="banner-warn">
          <span>文件因历史处理记录被跳过，可重新解压或清除处理记录。</span>
          <div className="banner-actions">
            <button
              className="btn-ghost"
              onClick={() => {
                void onForgetProcessed();
                onDismissSkipNotice();
              }}
              disabled={running}
            >
              清除记录
            </button>
            <button
              className="btn-primary sm"
              onClick={() => onReextractAllSkipped(skipNotice)}
              disabled={running}
            >
              全部重新解压
            </button>
          </div>
        </div>
      )}

      {/* 主队列卡片 */}
      <section className="card grow">
        {/* 临时密码输入 + 主控制栏 */}
        <div className="queue-header">
          <div className="pw-inline">
            <div className="pw-field">
              <input
                id="pw-input"
                className={"input-clean" + (!showPw && CSS_MASK_SUPPORTED ? " masked" : "")}
                type={showPw || CSS_MASK_SUPPORTED ? "text" : "password"}
                autoComplete="off"
                spellCheck={false}
                placeholder="本次解压密码（留空则自动尝试密码库）"
                value={password}
                onChange={(e) => onPasswordChange(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === "Enter") onStart();
                }}
              />
              <button
                type="button"
                className="icon-btn"
                onClick={() => setShowPw((v) => !v)}
                title={showPw ? "隐藏密码" : "显示密码"}
                aria-label={showPw ? "隐藏密码" : "显示密码"}
              >
                <EyeIcon off={showPw} />
              </button>
            </div>
            <label className="pill-check" title="勾选后，本次输入的密码会明文保存到本机密码库">
              <input
                type="checkbox"
                checked={rememberPassword}
                onChange={(e) => onRememberPasswordChange(e.target.checked)}
              />
              <span>记住到密码库</span>
            </label>
            <button className="btn-ghost" onClick={onOpenPasswords}>
              密码库 ({passwordCount})
            </button>
          </div>

          <div className="toolbar">
            <div className="toolbar-left">
              <button className="btn-primary" onClick={onStart} disabled={running || queuedTasks.length === 0}>
                开始解压{queuedTasks.length > 0 ? ` (${queuedTasks.length})` : ""}
              </button>
              {running && (
                <>
                  {paused ? (
                    <button className="btn-soft" onClick={onResume}>
                      继续
                    </button>
                  ) : (
                    <button className="btn-soft" onClick={onPause}>
                      暂停
                    </button>
                  )}
                  <button className="btn-danger-soft" onClick={onCancel}>
                    取消
                  </button>
                </>
              )}
              <button
                className="btn-soft"
                onClick={onRetryFailed}
                disabled={running || retryableTasks.length === 0}
              >
                重试异常{retryableTasks.length > 0 ? ` (${retryableTasks.length})` : ""}
              </button>
              <button className="btn-ghost" onClick={onClearList} disabled={running || tasks.length === 0}>
                清空
              </button>
            </div>
            <span className="muted small mono-ellipsis">
              {settings.outputRoot ? `输出：${settings.outputRoot}` : "输出：原文件所在目录"} · 并发 {settings.jobs}
            </span>
          </div>
        </div>

        {/* 进度与汇总 */}
        {(running || summary) && (
          <div className="progress-strip">
            <div className="progress big" title={batchProgress.known ? batchProgress.percent + "%" : "进度未知"}>
              <div className="bar" style={{ width: batchProgress.percent + "%" }} />
              <span className="pct">{batchProgress.known ? batchProgress.percent + "%" : "—"}</span>
            </div>
            <div className="progress-meta">
              <span className="activity" title={activity}>
                {running ? (paused ? "已暂停" : activity || "处理中…") : "批次已完成"}
              </span>
              {(summary || counts.total > 0) && (
                <div className="stat-pills">
                  <span className="stat ok">成功 {counts.success}</span>
                  {counts.skipped > 0 && <span className="stat">跳过 {counts.skipped}</span>}
                  {counts.needs_password > 0 && <span className="stat warn">需密码 {counts.needs_password}</span>}
                  {counts.failed > 0 && <span className="stat bad">失败 {counts.failed}</span>}
                  {counts.cancelled > 0 && <span className="stat">取消 {counts.cancelled}</span>}
                  <span className="muted small">
                    {humanBytes(counts.bytes)}
                    {summary ? ` · ${(summary.elapsed_ms / 1000).toFixed(1)}s` : ""}
                  </span>
                </div>
              )}
            </div>
          </div>
        )}

        {/* 任务表格 */}
        <div className="tablewrap">
          <table className="queue">
            <thead>
              <tr>
                <th className="col-name">文件</th>
                <th className="col-size">大小</th>
                <th className="col-status">状态</th>
                <th className="col-prog">进度</th>
                <th>结果</th>
                <th className="col-actions">操作</th>
              </tr>
            </thead>
            <tbody>
              {tasks.length === 0 && (
                <tr>
                  <td className="empty" colSpan={6}>
                    队列为空 — 拖入文件或点击上方「选择文件」开始
                  </td>
                </tr>
              )}
              {tasks.map((task) => (
                <TaskRow
                  key={task.key}
                  task={task}
                  running={running}
                  onRetry={onRetryOne}
                  onReextract={onReextractOne}
                  onReveal={onReveal}
                  onInspect={onInspect}
                  onRemove={onRemoveTask}
                  onToggleWarnings={onToggleWarnings}
                />
              ))}
            </tbody>
          </table>
        </div>

        <div className="card-footer">
          <span className="muted small">
            {queuedTasks.length > 0 ? `待处理 ${queuedTasks.length} 项 (${humanBytes(queuedSize)})` : "无待处理任务"}
          </span>
          <span className="muted small mono-ellipsis" title={recentLog}>
            {recentLog || "—"}
          </span>
        </div>
      </section>
    </div>
  );
}
