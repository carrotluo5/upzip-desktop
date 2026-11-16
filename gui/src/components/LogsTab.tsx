import { useEffect, useMemo, useRef, useState } from "react";

interface LogsTabProps {
  logs: string[];
  canOpenDir: boolean;
  onCopy: () => void;
  onClear: () => void;
  onOpenDir: () => void;
}

export function LogsTab({ logs, canOpenDir, onCopy, onClear, onOpenDir }: LogsTabProps) {
  const [filter, setFilter] = useState("");
  const boxRef = useRef<HTMLDivElement | null>(null);

  const visibleLogs = useMemo(() => {
    const q = filter.trim().toLowerCase();
    if (!q) return logs;
    return logs.filter((line) => line.toLowerCase().includes(q));
  }, [logs, filter]);

  useEffect(() => {
    if (boxRef.current && !filter.trim()) {
      boxRef.current.scrollTop = boxRef.current.scrollHeight;
    }
  }, [logs, filter]);

  return (
    <div className="page">
      <section className="card grow">
        <div className="card-head">
          <div>
            <h3>运行日志</h3>
            <p className="muted small">共 {logs.length} 条记录，同步写入本地日志目录</p>
          </div>
          <div className="btn-group">
            <input
              type="search"
              className="input-search"
              placeholder="过滤日志…"
              value={filter}
              onChange={(e) => setFilter(e.target.value)}
            />
            <button className="btn-soft" onClick={onCopy} disabled={logs.length === 0}>
              复制
            </button>
            <button className="btn-soft" onClick={onClear} disabled={logs.length === 0}>
              清空
            </button>
            <button className="btn-soft" onClick={onOpenDir} disabled={!canOpenDir}>
              打开日志目录
            </button>
          </div>
        </div>
        <div className="logbox grow" ref={boxRef}>
          {visibleLogs.length === 0 ? (
            <div className="empty-box">{logs.length === 0 ? "暂无日志" : "没有匹配的日志行"}</div>
          ) : (
            visibleLogs.map((line, index) => (
              <div className="logline" key={index}>
                {line}
              </div>
            ))
          )}
        </div>
      </section>
    </div>
  );
}
