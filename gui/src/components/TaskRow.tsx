import { Fragment, memo } from "react";
import { Task } from "../types";
import { humanBytes, kindLabel, passwordStateLabel, stageLabel, statusLabel } from "../utils";

export interface TaskRowProps {
  task: Task;
  running: boolean;
  onRetry: (path: string) => void;
  onReextract: (path: string) => void;
  onReveal: (path: string) => void;
  onInspect: (path: string) => void;
  onRemove: (key: string) => void;
  onToggleWarnings: (key: string) => void;
}

export const TaskRow = memo(function TaskRow({
  task,
  running,
  onRetry,
  onReextract,
  onReveal,
  onInspect,
  onRemove,
  onToggleWarnings,
}: TaskRowProps) {
  const detection = task.detection ?? null;
  const size = task.size ?? detection?.size ?? null;
  const warnings = task.warnings ?? [];
  const open = task.warningsOpen === true && warnings.length > 0;
  const rowPercent =
    task.percent != null
      ? task.percent
      : task.phase === "done" && (task.status === "success" || task.status === "skipped")
        ? 100
        : 0;
  const stateClass =
    task.phase === "running"
      ? "st-run"
      : task.phase === "queued"
        ? "st-queued"
        : "st-" + (task.status ?? "done");
  const badge =
    task.phase === "queued"
      ? "待处理"
      : task.phase === "running"
        ? stageLabel(task.stage)
        : statusLabel(task.status);
  const isRetryable =
    task.phase === "done" &&
    (task.status === "failed" || task.status === "needs_password" || task.status === "cancelled");
  const isReextract = task.phase === "done" && (task.status === "success" || task.status === "skipped");
  const notes = detection ? detection.notes.join("；") : "";

  return (
    <Fragment>
      <tr className={stateClass}>
        <td className="name" title={task.path}>
          <span className="name-text">{task.name}</span>
          {detection ? (
            <span className="submeta" title={notes}>
              {kindLabel(detection.kind)}
              {detection.format ? ` · ${detection.format}` : ""}
              {detection.password_state === "needs_password" ? " · 需密码" : ""}
            </span>
          ) : null}
        </td>
        <td className="size">{size != null ? humanBytes(size) : <span className="muted">—</span>}</td>
        <td className="status">
          <span className={"badge " + stateClass}>{badge}</span>
          {task.line ? (
            <span className="linemini muted" title={task.line}>
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
              {task.status === "success" ? ` · ${task.reason}` : task.reason}
            </span>
          ) : null}
          {task.status === "skipped" && !task.reason ? <span className="muted">已跳过</span> : null}
          {!task.status && detection ? (
            <span className="muted">{passwordStateLabel(detection.password_state)}</span>
          ) : null}
          {warnings.length > 0 ? (
            <button className="btn-ghost warnbtn" onClick={() => onToggleWarnings(task.key)}>
              警告 {warnings.length}
            </button>
          ) : null}
        </td>
        <td className="rowactions">
          {task.phase === "queued" && (
            <>
              <button className="btn-ghost" onClick={() => onInspect(task.path)}>
                探测
              </button>
              <button className="btn-ghost" disabled={running} onClick={() => onRemove(task.key)}>
                移除
              </button>
            </>
          )}
          {task.phase === "done" && (
            <>
              {isRetryable && (
                <button className="btn-ghost accent" disabled={running} onClick={() => onRetry(task.path)}>
                  重试
                </button>
              )}
              {isReextract && (
                <button className="btn-ghost" disabled={running} onClick={() => onReextract(task.path)}>
                  重解
                </button>
              )}
              {task.dest ? (
                <button className="btn-ghost" onClick={() => onReveal(task.dest ?? "")}>
                  定位
                </button>
              ) : null}
              <button className="btn-ghost" disabled={running} onClick={() => onRemove(task.key)}>
                移除
              </button>
            </>
          )}
        </td>
      </tr>
      {open && (
        <tr className="warnrow">
          <td colSpan={6}>
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
});
