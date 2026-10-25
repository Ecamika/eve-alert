# Linux PipeWire 捕获

入口为 `PipeWireScreencast`，实现上层 `Screencast` trait，不依赖 Tauri。
需在保持运行的 Tokio runtime 中调用。捕获通过系统 Portal 授权，
每次 `start` 创建新的单选或多选会话；Portal v4 起可申请持久化并传入恢复凭据。
后端返回新凭据，实际保存与更新由调用方负责。

```rust,no_run
use app_lib::platform::screencast::{
  pipewire::PipeWireScreencast, CaptureMultiplicity, CaptureOptions, CaptureTarget, CursorMode,
  Screencast, SelectionKind,
};
use futures_util::StreamExt;

async fn capture() -> Result<(), Box<dyn std::error::Error>> {
  let backend = PipeWireScreencast::new(None);
  let capabilities = backend.capabilities().await?;
  // 调用方应据能力选择界面选项；不支持的请求将返回 Unsupported。
  println!("{capabilities:?}");
  let mut session = backend.start(CaptureOptions::new(
    CaptureTarget::Picker(SelectionKind::Display),
    CursorMode::Hidden,
    Some(30),
  ).with_multiplicity(CaptureMultiplicity::Multiple)).await?;
  println!("选中来源：{:?}", session.sources());
  // 先复制 ID，避免遍历 sources() 的借用与 take_frames 的可变借用冲突。
  let ids: Vec<_> = session.sources().iter().map(|source| source.id.clone()).collect();
  let mut tasks = Vec::new();
  for id in ids {
    let mut frames = session.take_frames(&id).expect("每个来源首次取流");
    tasks.push(tokio::spawn(async move {
      // 示例各消费一帧；实际调用方可在独立任务中持续消费。
      match frames.next().await {
        Some(Ok(frame)) => {
          println!("来源 {:?}：{}×{}，序号 {}", frame.source_id,
            frame.image.width(), frame.image.height(), frame.sequence);
          Ok(())
        }
        Some(Err(error)) => Err(error),
        None => Ok(()),
      }
      // 此任务结束时丢弃 frames，只停止自己的来源。
    }));
  }
  let results = futures_util::future::join_all(tasks).await;
  // 包括帧流异常在内，仍等待整组原生线程退出及 Portal 会话关闭。
  session.stop().await?;
  for result in results {
    result??;
  }
  Ok(())
}
```

有宿主窗口时向 `new` 传入 Portal 窗口标识，例如 `x11:123abc`。
Wayland 标识需要宿主先导出并在整个授权过程中保活；后端不自行导出 Tauri 窗口。

## 一次多选

默认单选；`.with_multiplicity(CaptureMultiplicity::Multiple)` 让 `SelectSources`
发送 `multiple=true`，一次系统选择器操作允许选中多个窗口、多个显示器，或在
`SelectionKind::Any` 下混合选择。实际选择列表由 Portal 返回，多选也允许只选一个。
`multiple_picker_kinds` 声明后端支持请求的组合，系统选择器的实际交互由桌面实现决定。

`Start` 返回的全部 streams 都会处理，不截取第一个。每个来源建立一个原生 stream，
共用一个授权 remote、core、主循环和专用线程；全部连接后才报告启动成功。
任何来源不满足过滤、缺少必要 serial、重复或连接失败，都使整组启动失败。

`session.sources()` 返回来源列表，`session.take_frames(&source_id)` 分别取出每路流。
每个来源只能取出一次，未知 ID 或重复取出返回 `None`。各路有独立邮箱、唤醒器、
格式、帧序号、时间归一化和限速状态，并最多保留一个待消费图像。
来源共用会话时钟，但不承诺同步帧；不自动合成图像。

`stop()`、丢弃会话、Portal 会话关闭或共享 core 故障（包括权限撤销）停止整组。
成功启动后，来源节点删除、stream 断开、stream 错误、格式协商或帧处理错误
只结束对应来源，在该路交付一次终止错误，其他来源继续采集。
丢弃某路帧流只正常停止该来源，原生线程先撤销对应监听再销毁该 stream，
其他来源继续使用共享 remote。全部来源停止后关闭整组会话。
快照仍包含已停止的来源，不能再次取流或重启。

## 恢复与持久化

默认构造的选项不申请持久化，仍使用系统选择器。需要记住选择时，使用
`.with_persistence(CapturePersistence::UntilRevoked)`；成功后立即调用
`session.take_restore_token()`，保存返回的完整 `CaptureRestoreToken`。
下次使用 `CaptureTarget::RestoreOrPicker { token, kind }` 并再次申请所需范围。
首次与后续启动的统一示例见 [通用接口说明](../README.md#恢复凭据与持久化)。
一次多选只产生一个整组恢复凭据。恢复时也设置 `Multiple`，并检查
`multiple_restore_kinds`；来源 ID 和顺序不承诺跨恢复稳定。

- Portal v4 起，`restore_kinds` 与支持的选择器过滤一致，`persistence_modes`
  包括 `None`、`Application` 和 `UntilRevoked`，映射 `persist_mode` 的 0、1、2。
  这是协议支持的请求范围，不保证当前桌面后端或用户允许持久化。
- 旧版 Portal 只支持 `None`，恢复及持久化请求在创建会话前返回 `Unsupported`；
  普通选择器请求不会发送 v4 专有选项。
- `SelectSources` 传入旧 `restore_token`，`Start` 响应中的新 token 随成功启动的
  `CaptureSession` 交付。缺少 token 表示未获得可交付的恢复凭据，捕获仍可成功。
- token 一次性有效，克隆不能重复使用。同一份凭据的启动需串行，提交前移除
  可用存储中的旧凭据，成功后用新凭据替换。`Application` 范围只在内存保留。
- 失效、撤销或来源消失时，Portal 可以忽略旧 token 并正常提示用户重新选择；
  不提供“恢复失败也不弹窗”的保证，也不声称能判断本次是否成功恢复。
- 格式错误或后端不匹配的凭据在连接 D-Bus 前返回 `InvalidRestoreToken`，
  不自动打开选择器。新凭据的负载为 Portal 字符串，格式版本为 1，业务层不解析。
- 若 Portal 已返回新凭据，但后续 remote 或 PipeWire 启动失败/取消，凭据可能
  无法交付，旧凭据也可能已消耗；当前接受下次重新授权。停止会话不撤销持久授权。

## 能力与格式

- `capabilities` 读取 Portal 版本及来源/光标属性，不打开选择器。
- 支持系统公布的显示器、窗口过滤与 Hidden/Embedded 光标模式。
  混合选源需能够确认实际类型；Portal v3 之前同时支持两种来源时不声明 `Any`。
- Portal 不提供来源枚举或任意 ID 直接选源，这两种调用返回 `Unsupported`。
  `sources()` 中的 ID 是会话元数据标识，不能用于重新选源；平台未返回标题时标题为空。
- 优先使用 Portal v6 的 `pipewire-serial`；旧版本使用授权 remote 中的 node ID，
  禁止自动重连，并监控目标删除事件。不会捕获系统默认视频来源。
- 只协商 CPU MemPtr/MemFd 的 RGBA、BGRA、RGBx、BGRx 原始视频。
  支持 chunk offset、行填充、负 stride、裁剪、八种 SPA 方向、非预乘 alpha 输出。
  图像在归还原生缓冲区之前复制到自有内存。
- 明确声明的 HDR、非 sRGB 色彩空间、有限范围、隔行扫描、DMA-BUF/modifier
  返回错误。未声明色彩属性的 SDR 桌面流按 sRGB 解释。
  单边最多 16384 像素，单帧最多 256 MiB；超限返回错误。
- 每路流最多缓存一个图像，仅覆盖自己的旧帧；消费与唤醒互不影响。
  回调遇到本路缓存锁竞争会丢帧，`max_fps` 分别对各来源限速，不生成重复帧。

## 来源名称

Portal `Start` 的标准 streams 元数据不保证包含窗口标题或显示器名称。
后端从已授权目标的 registry 属性和完整 `NodeInfo` 中尽力读取名称，优先级为
`media.title`、`node.description`、`media.name`、`node.nick`；空值及纯空白值忽略。
后收到的低优先级属性或空值不会覆盖已有的有效名称。

只查询与 Portal 指定的 serial/node ID 匹配的来源节点，不使用本应用创建的
消费 stream 名称、`node.name` 或 `application.name` 充当窗口标题。
这些属性是平台提供的流名称或描述，未必是窗口真实标题；桌面没有提供名称或
没有读取权限时保持 `title == ""`，不生成展示标签，也不使捕获失败。

启动阶段通过两次 core roundtrip 完成初始 registry 枚举及选中节点的信息查询，
再把填充后的 `CaptureSource` 快照传回会话。名称是可选元数据，不等待非空名称出现。
会话的来源列表仍是启动时快照，不跟踪之后的窗口标题变化。

## 生命周期

`portal.rs` 管理 D-Bus 请求及授权会话，`runtime.rs` 将所有 PipeWire 原生对象
保留在专用线程，`buffer.rs` 读取 SPA 缓冲区与元数据，`rgba.rs` 完成像素转换、
时间归一化和限速，`session.rs` 管理整组控制句柄、来源停止状态与各路独立帧流。

启动取消会通知独立清理任务，关闭待处理请求、关闭 Portal 会话并等待原生线程。
派发中的 D-Bus 方法先取得结果再清理，方法超时为 10 秒；选择器等待用户操作时
不设置总超时。整组 PipeWire 来源连接超时为 10 秒。

丢弃会话请求整组停止，丢弃帧流只请求对应来源停止；`stop` 可以重复或并发调用，等待线程 join 和 Portal
清理，无需消费者轮询。清理失败可再次 `stop`，任务也会自动重试。
应用退出前应等待 `stop`；Tokio runtime 被宿主直接销毁时无法继续异步清理。
正常停止丢弃图像；来源异常仅向本路交付一次错误，共享会话异常向每路仍活动的流
各交付一次错误。后续清理保留各路已有错误；原始错误链通过 `Arc` 共享。

## 编译与验证

Linux Cargo 依赖使用 `pipewire 0.10.1` 和 `zbus 5.18`。当前锁定的
`pipewire`、`pipewire-sys`、`libspa`、`libspa-sys` 均为 0.10.1。
项目的 Rust 最低版本标记为 1.88，覆盖 zbus 要求的 1.87 和当前锁定的
image 0.25.10 要求的 1.88。
原生构建需要 pkg-config 能发现 `libpipewire-0.3`、`libspa-0.2`，以及 bindgen
可加载的 `libclang` 和相应 C 头文件。`flake.nix` 已提供 `pkg-config`、
`pipewire`、`clang` 和 `rustPlatform.bindgenHook`；hook 配置 `LIBCLANG_PATH`
及 `BINDGEN_EXTRA_CLANG_ARGS`。

2026-10-12，在更新后的 NixOS 开发环境 / Rust 1.95.0 上的验证：

- `cargo check --offline --locked --manifest-path src-tauri/Cargo.toml` 通过，
  包括原生 PipeWire 绑定及后端代码。
- `cargo test --offline --locked --manifest-path src-tauri/Cargo.toml platform::screencast`
  通过，完整工程内的 21 项屏幕捕获单元测试全部成功，未替换原生运行层。
- PipeWire 模块的 `rustfmt --check` 通过。
- 编译仍报告原有 `plugin/mod.rs` 的 `collections::HashMap` 未使用导入警告。
- 未启动系统选择器、未读取屏幕图像。权限允许/拒绝/撤销、用户取消、
  实际来源消失、动态尺寸变化及原生资源释放仍需在桌面会话中验证。

2026-10-13 增加恢复凭据后的验证：

- `cargo test --offline --locked --manifest-path src-tauri/Cargo.toml platform::screencast`
  通过：30 项成功，1 项桌面图像测试仍按原设置忽略。
- `cargo check --offline --locked --manifest-path src-tauri/Cargo.toml` 及
  `cargo clippy --offline --locked --manifest-path src-tauri/Cargo.toml --lib --tests`
  通过；仅报告原有 `plugin/mod.rs` 的未使用导入警告。
- 本次修改的 Rust 文件通过按 `src-tauri/.rustfmt.toml` 执行的格式检查，
  未对未修改模块应用格式化。
- 单元测试覆盖持久化参数映射、旧版 Portal 拒绝扩展请求、凭据序列化、
  旧凭据传入与新凭据解析、损坏/不匹配凭据拒绝、未获持久化批准以及一次性取出。
- 未打开系统选择器；跨重启恢复、凭据真正消耗与轮换、撤销后的重新授权，
  以及实际 PipeWire 采集仍需在桌面会话中验证。

同日增加一次多选后的验证：

- 同一条屏幕捕获测试命令通过：38 项成功，1 项桌面图像测试保持忽略。
- 离线且锁定依赖的 `cargo check`、Clippy（`--lib --tests`）及修改文件格式检查通过。
- 新增测试覆盖 `multiple=true`、全部来源解析、单选数量约束、混合来源与整组
  token 恢复、非法或重复来源导致整组失败、全部来源启动通知、各来源缓存与公平交付、
  以及整组停止时清空所有待消费帧。
- 未进行真实桌面授权、多窗口/多显示器采集或整组恢复测试；本次结果是
  编译与单元测试验证，不证明当前桌面的多选交互和原生运行结果。

同日改为各来源独立帧流后的验证：

- 同一条屏幕捕获测试命令通过：43 项成功，已有的 2 项桌面测试保持忽略。
- 离线且锁定依赖的 `cargo check`、Clippy（`--lib --tests`）及修改文件格式检查通过。
- 覆盖每个 ID 一次性取流、未知 ID、各路独立消费/缓存/唤醒、单路丢弃不影响其他路、
  最后一路停止触发整组清理、每路独立交付终止错误、停止后首次取流，以及原始错误链保留。
- 现有桌面测试已迁移到新接口，但未运行；单路原生 stream 的销毁及其他来源
  继续采集仍需真实桌面验证。

同日补充来源名称读取后的验证：

- 同一条屏幕捕获测试命令通过：47 项成功，2 项已有桌面测试保持忽略。
- 离线且锁定依赖的 `cargo check`、Clippy（`--lib --tests`）及修改文件格式检查通过。
- 新增测试覆盖名称优先级、空白/缺失名称保留、初始查询后的来源快照交付，
  以及 serial 不匹配时不能回退到已复用的 node ID。
- 未打开系统选择器或执行真实名称查询，当前 Niri 会话是否提供真实窗口标题
  仍需运行验证；编译与单元测试不保证特定桌面提供这些可选属性。

同日修复单路异常导致整组结束后的验证：

- 屏幕捕获测试通过：54 项成功，2 项已有桌面测试保持忽略。
- 离线且锁定依赖的 `cargo check` 和 Clippy（`--lib --tests`）通过。
- 新增 7 项回归测试，覆盖来源消失后其他路持续交付帧、独立唤醒、各路错误保留、
  最后一路异常后的清理、启动期间整组失败，以及 proxy/core 错误的处理范围。
- 未执行真实桌面授权或关闭窗口测试；若桌面 Portal 同时关闭整个授权会话，
  `SessionClosed` 仍会终止所有来源，单路隔离不能保持已被撤销的授权。

旧版 0.10.0 曾在测试构建时因生成绑定缺少 `SPA_ID_INVALID` 而失败。
升级到 0.10.1 后，本次编译和测试均未再出现该错误；两个 `-sys` 包均将
Clang 宏解析临时文件放在各自的 Cargo 输出目录。这个结果说明升级在当前
环境下消除了构建阻断，不单独证明旧错误的根因。

复验命令：

```sh
cargo check --offline --locked --manifest-path src-tauri/Cargo.toml
cargo test --offline --locked --manifest-path src-tauri/Cargo.toml platform::screencast
```

实现依据：[Portal ScreenCast 官方协议](https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.ScreenCast.html)
及 pipewire-rs 0.10.1 的 API 与 `streams` 示例。
来源名称字段语义见 [PipeWire 节点属性说明](https://docs.pipewire.org/page_man_pipewire-props_7.html)。
core 错误通知的 `id` 是发生错误的 proxy，见
[PipeWire core 事件说明](https://docs.pipewire.org/structpw__core__events.html)；
stream proxy 错误由 PipeWire 转为 stream 的 `state_changed(Error)` 通知。
