# EVE Alert

面向 EVE Online 的屏幕 RGB 识别与告警桌面应用。通过监测指定画面区域的颜色变化，为多个角色分别发出告警、告警解除和提醒。

项目聚焦画面颜色识别、监控配置和通知送达。使用 Tauri 2、Rust、React、TypeScript 和 MUI（Material Design）构建，当前屏幕捕获实现面向 Linux，依赖 XDG Desktop Portal 与 PipeWire。

## 功能与识别规则

- **多角色监控**：每个角色独立选择捕获来源、配置识别区域并维护识别状态。
- **区域与颜色配置**：截图框选、移动和八方向缩放选区，支持全屏编辑、缩放与平移；通过像素放大镜查看原始 RGB，也可手动输入坐标和多组 RGB。
- **界面主题**：支持浅色、深色和跟随系统；主题选择自动保存在当前 WebView 的本地存储中，不改变监控配置格式。
- **告警**：告警区域首次命中目标颜色时产生 `Warn`；持续命中不重复发送；全部目标颜色消失时产生一次 `WarnCleared`；再次出现时重新告警。
- **提醒**：依次检测“准备区域命中 → 敌人区域命中 → 敌人区域不再命中”，产生一次 `Reminder` 后重置。
- **通知渠道**：声音、Linux 桌面通知、ntfy、SSE、反向 WebSocket 和 OneBot 11 私聊。
- **排查工具**：导出原始 PNG 截图，查看最近 100 条事件及捕获、网络、通知和声音错误。

区域内任意像素精确匹配任意一种配置颜色即视为命中，忽略 alpha；坐标包含起点与终点。当前捕获帧率上限为 2 FPS，短于采样间隔的画面变化可能无法识别。界面颜色、缩放或布局变化后，应重新检查区域和取色配置。

各角色状态独立。启动时未命中不会发送告警解除；捕获异常或区域越界也不视为解除。画面缩小导致区域越界时，暂停该角色的识别并重置提醒状态，直到区域重新落入画面，或停止监控后重新配置。

## 开发与启动

仓库通过 `flake.nix` 描述 NixOS 开发环境。以下命令应在已配置的项目开发环境中执行：

```sh
pnpm exec tauri dev
```

仅运行 `pnpm dev` 可查看前端，端口为 `4567`；捕获、配置存储和监控需要 Tauri 桌面进程提供的 IPC。

| 命令 | 用途 |
| --- | --- |
| `pnpm build` | TypeScript 检查与前端构建，输出到 `dist/` |
| `pnpm lint` | 使用 Oxlint 检查前端 |
| `pnpm test` | 使用 Node 内置测试框架检查截图选区坐标与边界 |
| `pnpm exec tauri build` | 构建并打包桌面应用 |
| `cargo check --manifest-path src-tauri/Cargo.toml` | 检查 Rust 编译 |
| `cargo test --manifest-path src-tauri/Cargo.toml` | 运行 Rust 测试 |
| `cargo fmt --manifest-path src-tauri/Cargo.toml -- --check` | 检查 Rust 格式 |
| `cargo clippy --manifest-path src-tauri/Cargo.toml` | 检查 Rust 常见问题 |

Tauri 产品名为 `eve-alert`，应用标识符为 `dev.ecamika.project.eve-alert`。`frontendDist` 为相对 `src-tauri/` 的 `../dist/`，与前端构建输出对应。

## 使用流程

1. 添加角色并填写名称。名称用于标识事件，不会自动查找同名游戏窗口。
2. 点击“获取 / 恢复来源”，在系统屏幕共享选择器中选择该角色的 EVE 窗口或所在屏幕，确认共享授权。每个角色分别操作；后续优先恢复已保存的授权。更换窗口或屏幕时使用“重新获取来源”。
3. 收到首帧后点击“刷新截图”，配置告警、提醒准备、敌人出现三个区域。拖动框选区域，切换“取色”后点击画面取色；每个区域至少需要一种颜色。
4. 按需添加报告方式，点击“保存配置”，再“开始监控”。未添加报告方式时仍会记录事件。
5. 修改区域、颜色或其他配置前先停止监控。“停止监控”释放全部捕获资源，保留来源恢复凭据，下次启动监控时尝试恢复。

已连接的角色需先释放来源才能改名。恢复凭据按角色名称关联，改名后需要重新选源。“释放来源”停止当前捕获，但保留凭据。

截图编辑工具支持以下操作：

- **框选**：在选区外拖动创建新区域，在选区内拖动移动；拖动八个控制点调整大小。按住 Shift 拖动可在选区内重新框选。画布聚焦后，方向键移动 1 像素，按住 Shift 移动 10 像素；Esc 取消当前拖动。
- **取色**：切换“取色”后点击画面，将原始像素的 RGB 添加到当前区域，自动忽略重复颜色。放大镜与颜色读数来自原始截图，不受选框或遮罩影响；最后一种颜色不能删除。
- **缩放与平移**：使用放大、缩小、1:1、适应按钮，支持 12.5%–1600% 缩放；选择“平移”工具或按鼠标中键拖动画布，也可滚动查看。
- **精确调整**：直接编辑起点和终点坐标，输入限制为截图内的整数，自动保持起点不大于终点；已有配置超出新截图时显示提示，请重新框选或调整。
- **全屏编辑**：点击全屏按钮扩大编辑空间，退出按钮或 Esc 返回。监控期间锁定配置编辑，仍可查看、缩放和平移截图。

右上角可选择浅色、深色或跟随系统主题。仅运行前端开发服务器时会显示浏览器预览提示；桌面专用操作不可用。

声音由桌面 WebView 播放，同类音频播放期间不叠加。音频文件上限为 20 MiB，支持格式取决于 WebView 与系统解码器；告警解除使用“提醒 / 告警解除音频”。

## 配置、授权与截图

配置为 JSON，包含角色列表 `characters` 和通知配置 `report_methods`。前端启动时加载配置，保存时校验并通过同目录临时文件替换。无配置时显示空列表，首次保存才创建配置文件。

| 构建模式 | 配置路径 |
| --- | --- |
| Debug（`debug_assertions`） | 编译时项目根目录下的 `reporting.json` |
| Release | `~/.local/share/eve-alert/reporting.json` |

Debug 路径不依赖启动工作目录；Release 使用 `HOME/.local`。同目录的 `reporting-capture.json` 单独保存捕获恢复凭据，`screenshots/` 保存导出的截图，文件名包含角色名称和时间戳。开发配置、凭据和截图已加入 Git 忽略规则；配置可能含访问令牌，凭据文件不应分享或提交。

授权恢复依赖支持 ScreenCast v4 持久化能力的 Portal 后端。系统可能拒绝持久化或要求再次确认；应用在恢复前移除旧凭据，成功后保存新凭据，失败或取消后可能需要重新授权。

应用不自动迁移旧版配置目录，也不导入参考实现的 `settings.toml`。已有配置需手动放入当前配置路径；捕获授权以系统实际确认结果为准。

## 外部通知接入

### SSE

在前端配置监听 IPv4 地址与端口，监控启动时绑定服务。客户端连接 `http://监听地址:端口/events`：

```text
data: ["EVE - 角色1","EVE - 角色2"]

data: {"type":"Warn","title":"EVE - 角色1"}

data: {"type":"WarnCleared","title":"EVE - 角色1"}

data: {"type":"Reminder","title":"EVE - 角色1"}
```

首条数据为角色列表，后续为事件，每 5 秒发送保活注释；支持跨域 GET。可接入独立的 [eve-reporting-client](https://github.com/EVE-Little-God-People-Club/eve-reporting-client)。外部客户端需处理 `WarnCleared` 才能展示告警解除。

### ntfy

添加“ntfy”，填写服务器地址（默认 `https://ntfy.sh`）、主题和可选访问令牌；在 ntfy 客户端订阅相同服务器与主题。支持自建服务器。

应用以 JSON 发布事件，标题与正文包含状态和角色名称；告警优先级为 4，其余为 3。每次事件发送一次 HTTP 请求，失败时在界面显示错误，不自动重试；停止监控会取消尚未完成的请求。

### 反向 WebSocket

设置 `ws://` 或 `wss://` 地址，应用主动连接并发送与 SSE 示例相同的事件 JSON。可配置持续重连与重试间隔，连接中断期间不保证事件交付。

使用参考实现的独立 `onebot-reporting-bot` 时，填写该服务的 `/ws` 端点。

### OneBot 私聊

添加“OneBot 私聊”，填写 OneBot 11 正向 WebSocket 地址、可选访问令牌和私聊目标 ID，无需单独部署中转机器人。

应用调用 `send_private_msg`，文本为 `Warn 角色名称`、`WarnCleared 角色名称` 或 `Reminder 角色名称`。断线每 5 秒重连。告警去重由识别状态变化统一处理；旧配置字段 `warn_spacing_secs` 可被读取但不再生效。

## 代码结构

| 路径 | 职责 |
| --- | --- |
| `src/App.tsx`、`src/App.css` | 角色与来源配置、监控控制、页面布局与事件展示 |
| `src/theme.ts` | MUI 主题与组件默认样式 |
| `src/SnapshotEditor.tsx`、`src/selection.ts` | 截图编辑、取色与原始像素坐标计算 |
| `src/NotificationSettings.tsx` | 通知渠道配置表单 |
| `src/reporting.ts` | 前端业务类型与 IPC 封装 |
| `src-tauri/src/reporting/` | RGB 匹配、状态机、配置、捕获编排和通知输出 |
| `src-tauri/src/platform/screencast/` | 捕获契约、数据模型与 Linux PipeWire 实现 |
| `src-tauri/src/plugin/` | 内置插件与服务生命周期基础设施 |
| `src-tauri/src/lib.rs` | Tauri 初始化与业务命令注册 |

当前未实现 Windows 或 macOS 捕获后端，移动端振动等功能由外部客户端承担。现有基础设施不代表已提供对应产品功能。

## 验证范围

Rust 测试覆盖 RGB 精确匹配、像素边界、告警与提醒状态迁移、配置错误、授权恢复存储、事件协议及部分输出行为；捕获和插件基础设施另有模块测试。`pnpm test` 使用 Node 内置测试框架验证截图缩放坐标映射、反向拖动、选区移动与缩放边界，以及无效区域；当前未配置组件测试框架或覆盖率门槛。

真实捕获需在 Linux 桌面会话中授权，并用实际 EVE 画面验证三个区域、多角色状态及停止后的资源释放。声音、桌面通知和外部推送需结合实际桌面与服务联调。编译或自动测试通过不代表这些联调已完成。

## 参考实现

画面识别与报告功能参考 [eve-reporting](https://github.com/EVE-Little-God-People-Club/eve-reporting)，采用 RGB 精确匹配，并在桌面界面中配置识别规则与通知参数。
