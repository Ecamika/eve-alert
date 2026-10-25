# Repository Guidelines

## 项目结构与模块组织

本项目是 EVE Online 通用工具箱，处于起步阶段，当前重点为 Rust 后端。技术栈为 Tauri 2、React、TypeScript，计划使用 Tailwind，但尚未配置。

- `src-tauri/src/main.rs` 为程序入口，`lib.rs` 初始化 Tauri。
- `src-tauri/src/platform/` 放置平台能力；`screencast/` 包含捕获数据模型与 PipeWire 实现目录，`window/` 为窗口能力预留模块。
- `src-tauri/src/plugin/` 包含基于 `DashMap` 的服务注册表。
- `src/` 为前端源码，目前保留 Vite 示例；`src/assets/`、`public/` 为前端资源，`src-tauri/icons/` 为应用图标。
- `src-tauri/tauri.conf.json` 管理应用配置，`capabilities/` 管理权限；`flake.nix` 描述开发环境。

## 构建、测试与开发命令

以下命令供开发者在已配置的环境中按需执行：

| 命令 | 用途 |
| --- | --- |
| `pnpm dev` | 启动前端开发服务器，端口为 4567。 |
| `pnpm exec tauri dev` | 启动桌面应用及前端开发流程。 |
| `pnpm build` | 执行 TypeScript 检查并构建前端。 |
| `pnpm exec tauri build` | 构建并打包桌面应用。 |
| `pnpm lint` | 使用 Oxlint 检查前端。 |
| `cargo check --manifest-path src-tauri/Cargo.toml` | 检查 Rust 编译。 |
| `cargo test --manifest-path src-tauri/Cargo.toml` | 运行 Rust 测试。 |
| `cargo fmt --manifest-path src-tauri/Cargo.toml -- --check` | 检查 Rust 格式。 |
| `cargo clippy --manifest-path src-tauri/Cargo.toml` | 检查 Rust 常见问题。 |

打包前核对 `frontendDist` 与 Vite 实际输出目录；当前配置为 `../../dist/`。

## 代码风格与命名约定

遵循 `.editorconfig`：UTF-8、两空格缩进。Rust 格式遵循 `src-tauri/.rustfmt.toml`；模块、函数使用 `snake_case`，类型与 trait 使用 `PascalCase`。React 组件使用 `PascalCase`，变量与函数使用 `camelCase`。前端由 Oxlint 检查，TypeScript 已启用未使用变量检查。

平台实现放入对应子模块；共享数据类型与接口留在上层模块。避免将平台细节直接耦合到 Tauri 启动逻辑。

## 测试规范

当前没有已编写的测试、前端测试框架或覆盖率门槛。新增后端逻辑使用 Rust 内置测试框架，在模块内添加 `#[cfg(test)] mod tests`；集成测试放入 `src-tauri/tests/`。测试名称应描述行为，例如 `returns_none_for_unknown_service`。涉及屏幕捕获的改动应说明运行平台、授权过程及实际验证结果。

## 提交与 Pull Request

当前目录无法读取 Git 历史，因此没有可确认的既有提交规范。建议使用 `feat:`、`fix:`、`docs:`、`refactor:` 前缀，例如 `feat: 添加捕获来源模型`。

PR 应说明目的、影响模块、验证命令与结果，并关联相关 issue。界面改动附截图；平台功能注明操作系统与桌面会话。保留已有未提交修改，不提交 `node_modules/`、`target/`、`.direnv/` 等生成内容。

## 智能体与环境约束

使用中文交流及编写项目说明。`CLAUDE.md` 如需创建或维护，应指向本文件，避免重复规则。当前系统为 NixOS；不得通过命令修改系统状态或环境。环境疑问可只读查看 `/home/ecamika/nixos-config`，硬件信息可通过 `fastfetch` 查看。不要为解决依赖问题擅自安装软件或修改系统配置。

若因 `flake.nix` 提供的开发环境存在问题，导致编译、运行、测试或其他开发工作受阻，必须立即停止当前任务并向用户报告，不得擅自修复或绕过环境问题。报告应包含失败命令、关键错误信息及已确认的环境问题；若缺少环境依赖，必须列出缺失的工具、库或环境变量，无法确认的缺失项应明确标注为待确认。
