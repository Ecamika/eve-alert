# 项目协作指南

## 项目定位与范围

EVE Alert（`eve-alert`）是面向 EVE Online 的屏幕 RGB 识别与告警桌面应用。当前范围为多角色画面捕获、区域与颜色配置、告警和提醒状态识别，以及本地和远程通知。技术栈为 Tauri 2、Rust、React、TypeScript，界面使用 MUI（Material Design）、Emotion 与少量 CSS，支持浅色、深色和跟随系统主题；未配置 Tailwind。

开发应围绕现有监控流程开展。新增功能需说明与 RGB 识别或告警的关系；未经用户明确要求，不扩展为通用 EVE 工具箱，也不因现有插件基础设施而主动增加产品功能。按任务需要修改，避免无关清理或架构重写。

## 项目结构与职责

- `src/App.tsx`、`src/App.css`：角色与来源配置、监控控制、页面布局及事件展示。
- `src/theme.ts`：MUI 主题、浅色与深色配色及组件默认样式。
- `src/SnapshotEditor.tsx`、`src/selection.ts`：截图缩放、框选、移动与缩放选区、原始像素取色和坐标计算；提醒配置在同一个编辑页及全屏窗口中切换三个独立区域，同时显示三个选框。
- `src/NotificationSettings.tsx`：通知渠道配置表单。
- `src/reporting.ts`：前端业务类型与 Tauri IPC 封装；修改命令或序列化结构时同步检查此处。
- `src-tauri/src/main.rs`：程序入口；`lib.rs`：Tauri 初始化、状态管理与命令注册。
- `src-tauri/src/reporting/config.rs`：配置模型、校验、RGB 匹配、告警与提醒状态机、配置路径。
- `src-tauri/src/reporting/mod.rs`：业务命令、捕获编排、监控生命周期、事件分发、截图与音频读取。
- `src-tauri/src/reporting/capture.rs`：来源选择和恢复凭据存储；`outputs.rs`：SSE、WebSocket、桌面通知、ntfy 与 OneBot 输出。
- `src-tauri/src/platform/screencast/`：共享捕获接口和数据模型；`pipewire/`：Linux Portal / PipeWire 实现。
- `src-tauri/src/plugin/`：内置插件、服务契约与生命周期管理，具体约定见该目录的 `README.md`；当前业务由 Tauri 直接管理，勿假定已接入插件生命周期。
- `src-tauri/tauri.conf.json`、`capabilities/`：应用配置与权限；`flake.nix`：NixOS 软件包与开发环境；`public/`、`src/assets/`、`src-tauri/icons/`：静态资源与图标。

业务规则留在 `reporting/`，平台细节放入对应平台子模块，共享捕获契约留在 `screencast/` 上层。避免将识别逻辑或平台细节直接放入 Tauri 启动代码。

## 必须保持的业务行为

除非用户明确要求改变，下列行为应保持，并在修改相关逻辑时验证：

- 区域两端坐标均包含；任意像素的 RGB 精确匹配任意目标颜色即命中，忽略 alpha。不要默默引入容差或改变坐标语义。
- 各角色独立维护状态。告警首次命中产生 `Warn`，持续命中不重复；由命中转为未命中时产生 `WarnCleared`，启动时未命中不发送解除。
- 角色 `enabled` 缺省为 `true`，禁用状态持久保存。停止监控后调整启用状态；启动时至少启用一个角色，仅启用角色参与来源恢复、画面检查、识别与 SSE 初始角色列表，释放其他角色的预览捕获并保留凭据。覆盖已有角色配置只替换四个区域及其 RGB，保留目标角色名称、授权和启用状态，复制后区域数据独立。
- 捕获异常和区域越界不视为告警解除。越界时暂停该角色识别并重置提醒状态。
- 提醒的区域 1 / RGB1（`reminder_now_region`）、新增前置区域 2 / RGB2（`reminder_gate_region`）和原区域改为的区域 3 / RGB3（`reminder_enemy_region`）分别配置坐标与颜色。区域 1 首次命中时发送 `Reminder`（`stage: LastWave`、`message: 异常进入最后一波次`），随后检测区域 2，命中任意 RGB2 后才开启区域 3 检测，RGB2 后续消失不撤销前置记录。同帧满足前置条件后立即检测下一阶段。本轮开始前的 RGB2、前置命中前的 RGB3 不保留记录。RGB1 消失，或记录到 RGB3 后所有 RGB3 消失，发送一次 `Reminder`（`stage: Ended`、`message: 异常结束`），清空本轮标记。结束后如果 RGB1 仍在，等待它消失并重新出现才开始下一轮。两个条件同时满足只发送一次结束提醒，停止监控和无效帧重置不发送结束提醒。旧配置缺少 `reminder_gate_region` 时复制 `reminder_enemy_region` 为独立初始值，保留原区域及检测行为；界面可单独配置新增区域。
- 当前捕获帧率上限为 2 FPS。更改频率、匹配规则或状态迁移时，说明对漏检、重复告警和资源消耗的影响。
- RGB3（原 RGB2）支持多个目标色：任意一种命中即记录，无需全部出现；记录后按整个区域和完整颜色列表判断，所有配置颜色均不存在时才由 RGB3 消失触发结束，不能只跟踪最初命中的颜色或像素。RGB1 消失仍独立触发结束。
- 停止监控应关闭输出任务并释放捕获资源；保留恢复凭据。捕获失败、取消、重复启停等路径也需正确清理。
- 事件类型 `Warn`、`WarnCleared`、`Reminder` 和 JSON 字段属于外部接入协议；变更时检查所有输出渠道、前端类型、测试和 README，说明兼容性影响。

## 配置与数据

Debug 配置为编译时项目根目录的 `reporting.json`；Release 为 `$HOME/.local/share/eve-alert/reporting.json`，当前不使用 XDG 配置目录覆盖。恢复凭据为同目录的 `reporting-capture.json`，导出截图位于 `screenshots/`。

配置和恢复凭据通过同目录临时文件替换；保留现有校验、Unix 文件权限与凭据轮换语义。凭据按角色名称关联，不要把 Portal node ID 当作可持久化授权。不要在日志、示例、截图或提交中暴露访问令牌和捕获凭据。

产品名为 `eve-alert`，显示名称为 `EVE Alert`，Tauri 标识符为 `dev.ecamika.project.eve-alert`。应用标识符和数据路径的变动需要考虑已有配置与授权，不能假定系统或应用会自动迁移。

## 构建与验证命令

以下命令在已配置的项目开发环境中按需执行：

| 命令 | 用途 |
| --- | --- |
| `pnpm dev` | 启动前端开发服务器，端口为 4567；桌面能力仍需 Tauri IPC |
| `pnpm exec tauri dev` | 启动桌面应用及前端开发流程 |
| `pnpm build` | TypeScript 检查与前端构建 |
| `pnpm exec tauri build` | 构建并打包桌面应用 |
| `pnpm lint` | 使用 Oxlint 检查前端 |
| `pnpm test` | 使用 Node 内置测试框架检查截图选区坐标、边界与角色配置覆盖 |
| `cargo check --manifest-path src-tauri/Cargo.toml` | 检查 Rust 编译 |
| `cargo test --manifest-path src-tauri/Cargo.toml` | 运行 Rust 测试，可追加模块过滤条件 |
| `cargo fmt --manifest-path src-tauri/Cargo.toml -- --check` | 检查 Rust 格式 |
| `cargo clippy --manifest-path src-tauri/Cargo.toml` | 检查 Rust 常见问题 |

Vite 输出目录为项目根目录 `dist/`，Tauri 的 `frontendDist` 为 `../dist/`（相对 `src-tauri/`）。涉及构建配置时核对两者一致。

## 代码风格与测试

遵循 `.editorconfig`：UTF-8、两空格缩进；Rust 格式遵循 `src-tauri/.rustfmt.toml`。Rust 模块与函数使用 `snake_case`，类型与 trait 使用 `PascalCase`；React 组件使用 `PascalCase`，变量与函数使用 `camelCase`。前端由 Oxlint 检查，TypeScript 已启用未使用变量检查。

现有 Rust 测试分布在 `reporting/`、`platform/screencast/` 和 `plugin/` 等模块，前端选区和角色配置纯逻辑测试位于 `tests/`，使用开发环境提供的 Node 内置测试框架；无组件测试框架或覆盖率门槛。测试聚焦关键业务行为、边界条件、错误处理和缺陷回归，禁止添加过多无用测试；避免重复覆盖、照搬实现，或仅验证简单赋值与字段读写。不为凑数量或覆盖率增加测试。

新增后端测试沿用 Rust 内置测试框架与现有模块组织；集成测试可放入 `src-tauri/tests/`。名称描述行为，例如 `returns_none_for_unknown_service`。按改动风险选择检查；纯文档修改无需运行完整构建或桌面捕获。

涉及识别逻辑时关注边界、多角色隔离、告警去重与解除、提醒迁移和无效帧。涉及捕获时关注授权恢复、取消或失败清理、尺寸变化和资源释放。涉及输出时检查事件协议、断线和失败行为，避免引入重复推送。

屏幕捕获改动的报告应注明操作系统、桌面会话、授权过程和实际验证结果。声音、桌面通知、ntfy、WebSocket 与 OneBot 需相应环境联调；明确区分自动测试通过与真实桌面验证，未执行的验证应如实说明。

## 提交与 Pull Request

建议使用 `feat:`、`fix:`、`docs:`、`refactor:` 前缀，例如 `fix: 避免捕获异常触发告警解除`。

PR 说明目的、影响模块、用户可观察到的行为、验证命令与结果，并关联相关 issue。界面改动附截图；平台功能注明操作系统与桌面会话；配置或协议变更说明兼容性和迁移方式。行为变化时同步更新 README，协作规则或结构变化时同步更新本文件。

保留已有未提交修改，不提交 `node_modules/`、`target/`、`.direnv/`、`dist/` 等生成内容，也不提交运行配置、捕获凭据或截图数据。

## 智能体与环境约束

使用中文交流及编写项目说明。`CLAUDE.md` 如需创建或维护，应指向本文件，避免重复规则。当前系统为 NixOS；不得通过命令修改系统状态或环境。环境疑问可只读查看 `/home/ecamika/nixos-config`，硬件信息可通过 `fastfetch` 查看。不要为解决依赖问题擅自安装软件或修改系统配置。

若因 `flake.nix` 提供的开发环境存在问题，导致编译、运行、测试或其他开发工作受阻，必须立即停止当前任务并向用户报告，不得擅自修复或绕过环境问题。报告应包含失败命令、关键错误信息及已确认的环境问题；若缺少环境依赖，必须列出缺失的工具、库或环境变量，无法确认的缺失项应明确标注为待确认。
