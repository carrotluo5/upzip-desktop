# 构建（BUILD）— macOS / Linux

本文讲 **Unparanoia 0.3.1** 的构建与产物；Windows 详见 [WINDOWS_BUILD.md](WINDOWS_BUILD.md)。
0.3.1 起 Windows 便携包改为**单 exe**（GUI/CLI 内嵌 7-Zip 与前端，不再附 sidecar）。
项目是单个 Cargo workspace + 一个 Vite/React 前端，`gui/src-tauri` 是 workspace 成员，
所以**所有 Rust 产物都在仓库根目录的 `target/` 下**（不是 `gui/src-tauri/target`，
旧版文档里那个路径已过时）。

## 1. 依赖

| 依赖 | 版本要求 | 检查命令 |
|---|---|---|
| Rust | >= 1.80（实测 1.99.0） | `cargo --version` |
| Node.js | >= 20（实测 v26.10.0） | `node --version` |
| pnpm | 11.x（实测 11.27.0） | `pnpm --version` |
| Xcode Command Line Tools | macOS 构建必需 | `xcode-select -p` |
| WebKit / GTK 等 | Linux 构建需要（Tauri 官方前置） | 见 Tauri docs |

前端依赖已用 pnpm 管理；`gui/pnpm-lock.yaml` **已提交**，构建用 `--frozen-lockfile`。

## 2. 一键构建（macOS）

```bash
./scripts/build.sh
```

脚本依次做（任何一步失败即退出）：

1. 环境检查（cargo/node/pnpm、Xcode CLT、`binaries/mac/7zz` 存在）；
2. `cargo test --workspace` —— core 单测 + oracle 向量回放 + embedded-7z 单测 +
   unpack-core 单测（v0.3.1 当前 107 通过 / 0 失败 / 2 ignored，最终以
   VERIFICATION.md §2 为准；`real_sample` 2 个需要真实样本的测试默认 ignored）；
3. `pnpm --dir gui install --frozen-lockfile`；
4. `pnpm --dir gui tauri build`（`beforeBuildCommand` 自动跑
   `pnpm sync:binaries && pnpm build`，把前端 dist 与 7-Zip 资源打进 .app）；
5. `cargo build --release -p unparanoia-cli`（CLI 交付件，一次构建同时产出 GUI + CLI）；
6. 打印 dmg / .app / 裸二进制 / CLI 的大小与 SHA-256。

也可以手动分步：

```bash
cargo test --workspace
pnpm --dir gui install --frozen-lockfile
pnpm --dir gui tauri build            # 或: pnpm --dir gui tauri build --no-bundle
cargo build --release -p unparanoia-cli
```

## 3. 产物（workspace 根 target/release）

| 产物 | 路径 | 说明 |
|---|---|---|
| GUI .app | `target/release/bundle/macos/Unparanoia.app` | 内含 `Contents/Resources/binaries/mac/7zz` 与许可证 |
| DMG | `target/release/bundle/dmg/Unparanoia_<version>_aarch64.dmg` | 文件名架构由构建机决定 |
| GUI 裸二进制 | `target/release/unparanoia` | 调试/分发用 |
| CLI | `target/release/unparanoia-cli` | `cargo build --release -p unparanoia-cli` 产生 |
| 前端 dist | `gui/dist/` | `pnpm --dir gui build` 产生，构建时自动生成 |

DMG 里的 .app 是本次构建的完整拷贝；`target/` 与 `gui/dist/` 都已 gitignore。

## 4. 签名与分发（诚实边界）

- 本地 `tauri build` 产出的 .app 只有 **ad-hoc/linker 签名**，
  `codesign --verify --deep --strict` 会失败（实测报
  `code has no resources but signature indicates they must be present`）。
  这不影响本机运行，但**不能直接分发给别人**。
- 正式分发需要：Apple Developer ID Application 证书签名 + `notarytool` 公证 +
  staple；Tauri 支持通过环境变量 `APPLE_CERTIFICATE`、`APPLE_ID` 等完成，
  本仓库未配置（无证书）。
- 未开启 hardened runtime、未配置 entitlements；应用会启动 7-Zip 子进程，
  若将来启用 hardened runtime 需注意子进程与资源路径。
- DMG 未做自定义背景/布局，用 Tauri 默认 bundler。

## 5. 构建常见问题

| 现象 | 原因 / 处理 |
|---|---|
| `error: failed to run custom build command for ... tauri-build` | 前端 `gui/dist` 不存在或 `tauri.conf.json` 的 `frontendDist` 不对；先 `pnpm --dir gui build` |
| 图标/配置改了但 .app 还是旧图标 | 删 `target/release` 或 `touch gui/src-tauri/build.rs` 强制 tauri-build 重跑 |
| `pnpm install --frozen-lockfile` 失败 | 锁文件与 package.json 不一致：本地先 `pnpm --dir gui install` 更新并提交锁文件 |
| DMG 构建失败但 .app 已生成 | 常见于已挂载同名卷/残留 `bundle_dmg.sh`；先 `hdiutil info` 卸载旧卷再重试。**2026-10-24 本机实测的另一种根因（已定位）：**之前被超时杀掉/中断的构建在 `target/release/bundle/macos/` 留下同名半成品 `Unparanoia_<version>_aarch64.dmg`（512 B）和 `rw.<pid>.*.dmg`（各约 39.9 MB）；`bundle_dmg.sh` 以 cwd=`target/release/bundle/macos` 运行且用相对文件名，`hdiutil convert` 目标已存在 → `hdiutil: convert failed - 文件已经存在`，tauri 又把脚本 stderr 吞成 `failed to run bundle_dmg.sh`。修复：`rm -f target/release/bundle/macos/Unparanoia_<version>_aarch64.dmg target/release/bundle/macos/rw.*.dmg` 后重跑即成功（无需改代码）。另：同一工作区建议串行构建，避免残留 |
| GUI 在 Windows 上白屏 / 停在 `localhost:1420` | 裸 `cargo build` 构建 GUI 时漏了 `--features custom-protocol`（`tauri build` 与 `scripts/build-windows-*.{sh,ps1}` 会自动带）；见 WINDOWS_BUILD.md §4 |
| 端口 1420 被占用（dev 模式） | `pnpm --dir gui dev` 用的固定端口；杀掉占用进程 |

## 6. 开发模式

```bash
pnpm --dir gui install
pnpm --dir gui tauri dev      # 起 Vite dev server + Tauri 窗口，改前端热更新
```

`beforeDevCommand` 同样会跑 `pnpm sync:binaries`，所以开发时也需要
`binaries/mac/7zz` 存在（它由 git 跟踪，克隆即有）。

## 7. 实测记录与已知环境问题

**v0.3.1 最终构建记录（2026-10-24 18:06-18:08，macOS 27.2 arm64 本机，
`./scripts/build.sh` exit 0；其中 `cargo test --workspace` = 107 passed / 0 failed /
2 ignored）**：

| 产物 | 大小 (B) | SHA-256 |
|---|---|---|
| `target/release/bundle/dmg/Unparanoia_0.3.1_aarch64.dmg` | 8,212,544 | `c7ac5ade88d2d14d644cd99e629e2a52e8e1b4590929d9c2946f8adae723fd76` |
| `.app` 主二进制 = `target/release/unparanoia` | 7,398,032 | `4b689e1e18b32c43d442360987b6d952796ecc5f0957303e99161e5c8a032300` |
| `target/release/unparanoia-cli` | 2,789,152 | `348df56c88dff62482a4e45eb76f40ec5b676bf169b50b27b0779fcb6d78767d` |

DMG 挂载复验通过：含 `Applications` 链接与 `Unparanoia.app`（0.3.1、
`com.nanase.unparanoia`、minOS 12.0），`Contents/Resources/binaries/mac/7zz` 存在且哈希
与 `binaries/mac/7zz` 一致。`.app` 仍为 ad-hoc 签名（`codesign --verify --deep --strict`
失败），不能直接分发。

Windows 单 exe 便携 zip 与包内 exe 的哈希见 [VERIFICATION.md](VERIFICATION.md) §7 与
[WINDOWS_BUILD.md](WINDOWS_BUILD.md) §5。每次重新构建（即使源码相同）哈希都会变，
因为二进制/打包包含构建时间与路径。

> v0.3.0 的历史产物数字已过时，本文件不再保留，避免误引用。

**已知环境问题（2026-10-24，macOS 27.2 arm64，v0.2.0 构建时实测）——已定位，非代码 bug**：
DMG 打包曾失败，tauri 只回报 `failed to run bundle_dmg.sh`（脚本 stderr 被吞）。
用 `-vv` 抓到的真实错误是 `hdiutil: convert failed - 文件已经存在`：之前被超时
杀掉/中断的构建在 `target/release/bundle/macos/` 留下了半成品同名 dmg（512 B）和
4 个 `rw.<pid>.*.dmg`（每个约 39.9 MB）。`bundle_dmg.sh` 以
cwd=`target/release/bundle/macos` 运行、dmg 用相对文件名，`hdiutil convert`
目标已存在即失败。修复命令：

```bash
rm -f target/release/bundle/macos/Unparanoia_<version>_aarch64.dmg \
      target/release/bundle/macos/rw.*.dmg
```

清掉残留后重跑 `pnpm --dir gui tauri build --bundles dmg` 即可成功（v0.3.0 最终构建已
按此修复产出上表 DMG），无需 workaround、无需改代码。通用建议仍是：
同一工作区串行构建，避免构建被中断后留下半成品。
