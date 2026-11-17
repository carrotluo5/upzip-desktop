import { useState } from "react";
import { maskPassword, CSS_MASK_SUPPORTED } from "../utils";
import { EyeIcon } from "./Icons";
import { isPasswordEmpty } from "../task-logic";

interface PasswordsTabProps {
  passwords: string[];
  configPath: string;
  onRefresh: () => Promise<void>;
  onAdd: (value: string) => Promise<boolean>;
  onRemove: (index: number) => Promise<void>;
  onClear: () => Promise<void>;
}

export function PasswordsTab({
  passwords,
  configPath,
  onRefresh,
  onAdd,
  onRemove,
  onClear,
}: PasswordsTabProps) {
  const [input, setInput] = useState("");
  const [revealed, setRevealed] = useState<number[]>([]);

  async function handleAdd() {
    const ok = await onAdd(input);
    if (ok) setInput("");
  }

  function toggleReveal(index: number) {
    setRevealed((prev) => (prev.includes(index) ? prev.filter((v) => v !== index) : prev.concat(index)));
  }

  return (
    <div className="page">
      <section className="card grow">
        <div className="card-head">
          <div>
            <h3>密码库</h3>
            <p className="muted small">按顺序自动尝试每一层压缩包与容器；仅明文保存在本机</p>
          </div>
          <div className="btn-group">
            <button className="btn-soft" onClick={() => void onRefresh()}>
              刷新
            </button>
            <button className="btn-danger-soft" onClick={() => void onClear()} disabled={passwords.length === 0}>
              清空全部
            </button>
          </div>
        </div>

        <div className="pw-add-row">
          <input
            className={"input-clean" + (CSS_MASK_SUPPORTED ? " masked" : "")}
            type={CSS_MASK_SUPPORTED ? "text" : "password"}
            autoComplete="off"
            spellCheck={false}
            autoCorrect="off"
            autoCapitalize="off"
            placeholder="输入新密码（支持中文直接键入）"
            value={input}
            onChange={(e) => setInput(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter") void handleAdd();
            }}
          />
          <button className="btn-primary" onClick={() => void handleAdd()} disabled={isPasswordEmpty(input)}>
            添加密码
          </button>
        </div>

        <div className="pw-list">
          {passwords.length === 0 ? (
            <div className="empty-box">密码库为空 — 添加后将在解压时自动按顺序尝试</div>
          ) : (
            passwords.map((value, index) => {
              const isShown = revealed.includes(index);
              return (
                <div className="pw-item" key={index}>
                  <span className="pw-idx">#{index + 1}</span>
                  <span className="pw-val">{isShown ? value : maskPassword(value)}</span>
                  <div className="btn-group">
                    <button
                      className="btn-ghost"
                      onClick={() => toggleReveal(index)}
                      title={isShown ? "隐藏明文" : "显示明文"}
                    >
                      <EyeIcon off={isShown} />
                      <span>{isShown ? "隐藏" : "显示"}</span>
                    </button>
                    <button
                      className="btn-ghost danger"
                      onClick={() => {
                        setRevealed((prev) => prev.filter((v) => v !== index));
                        void onRemove(index);
                      }}
                    >
                      删除
                    </button>
                  </div>
                </div>
              );
            })
          )}
        </div>

        <div className="card-footer">
          <span className="muted small mono-ellipsis" title={configPath}>
            存储位置：{configPath || "—"}
          </span>
          <span className="muted small">不联网 · 仅本地明文保存</span>
        </div>
      </section>
    </div>
  );
}
