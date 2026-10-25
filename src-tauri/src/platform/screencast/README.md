# 跨平台屏幕捕获接口

本目录定义捕获契约。`mod.rs` 定义后端与会话 trait，
`types.rs` 定义通用数据，`error.rs` 定义错误；这些类型仍从 `screencast` 顶层导出。
`pipewire/` 实现 Linux 的 Portal 授权、PipeWire 捕获与 CPU RGBA 帧流，
只在 Linux 上导出；调用方法与验证范围见 [PipeWire 后端说明](pipewire/README.md)。

## 调用流程

1. 创建平台后端，将宿主窗口标识、UI 调度器及平台上下文注入后端。
2. `capabilities().await` 查询枚举、直接选源、选择器、光标、恢复与持久化请求能力。
3. 支持枚举时调用 `sources().await`，或直接请求支持的系统选择器模式。
4. `start(options).await` 完成授权与选源，返回已启动的 `Box<dyn CaptureSession>`。
5. 需要记住选择时，立即用 `take_restore_token()` 返回的新凭据替换旧凭据；
   `sources()` 查看实际来源列表，用 `take_frames(&source_id)` 分别取出各来源的帧流。
6. 不同任务独立消费各路帧，需要确定整组资源已释放时等待 `stop().await`。

后端适合由 `Arc<dyn Screencast>` 共享。会话先通过可变引用分别取出帧流，之后可将
`Box<dyn CaptureSession>` 转为 `Arc<dyn CaptureSession>`，在另一任务中停止捕获。
帧流为 `'static + Send`，不借用控制句柄，因此等待下一帧不会阻止停止操作。

## 平台映射

下表描述平台映射，其中 Windows、macOS 尚未实现。能力根据实际系统版本与所用 API 查询，
不能仅根据操作系统名称硬编码。每帧来自一个显示器或窗口，支持多选的后端
可以在一次授权会话中捕获多个来源，但不自动合成图像。

| 平台 | 选源与授权 | 捕获会话与帧 | 约束 |
| --- | --- | --- | --- |
| Linux PipeWire | 桌面共享通过 XDG Desktop Portal 的 CreateSession → SelectSources → Start → OpenPipeWireRemote | 每个来源一个 PipeWire stream → 带来源 ID 的自有 RGBA 图像 | 支持一次单选或多选；共享授权 remote 和原生线程；不承诺枚举或任意 ID 直接选源 |
| Windows | Windows.Graphics.Capture 的系统选择器；直接选源可使用窗口/显示器互操作 API，并由后端实现来源枚举 | GraphicsCaptureSession 与 Direct3D 帧池 → 自有 RGBA 图像 | 后端注入选择器宿主 HWND，并处理 UI/COM 线程要求；选择器只声明真正支持的类型过滤模式 |
| macOS | ScreenCaptureKit 的 SCShareableContent 与 SCContentFilter；系统选择器通过 SCContentSharingPicker | SCStream 与屏幕输出回调 → 自有 RGBA 图像 | 按系统版本和所用 API 声明单选/多选能力；禁止会话内更换来源；处理主线程调度与授权 |

`picker_kinds` 的 `Any` 是独立能力：能够打开不带类型过滤的选择器，不代表
能够打开只选窗口或只选显示器的选择器。反过来也不隐式推导 `Any`。
`cursor_modes` 只声明后端能保证的最终图像行为；无法满足时必须报错。
平台专有 ID、授权句柄、GPU 对象与线程调度均留在具体后端内部。
恢复凭据通过通用 `CaptureRestoreToken` 传递自有数据，平台语义仍由后端解释。

## 一次选择多个来源

`CaptureOptions::new` 默认 `CaptureMultiplicity::Single`，恰好选择一个来源。
使用 `.with_multiplicity(CaptureMultiplicity::Multiple)` 允许一次选择一个或多个
来源；用户仍可只选择一个，不要求至少两个，也不能用重复弹出单选界面来模拟多选。
`SelectionKind::Window` 允许多个窗口，`Display` 允许多个显示器，`Any` 允许
显示器与窗口混合选择；实际支持的组合须查能力。

`multiple_picker_kinds` 和 `multiple_restore_kinds` 分别声明选择器多选和整组恢复
支持的过滤，是对应普通能力的子集。不支持的请求返回 `Unsupported`，不能
静默降级为单选或只保留首个来源。直接 `Source(id)` 仍然指定一个来源。

会话的 `sources()` 是非空且 ID 唯一的固定快照。每个来源拥有独立帧流，通过
`take_frames(&source_id)` 取出；每个有效 ID 首次返回 `Some`，重复取出或未知 ID
返回 `None`，不影响任何来源。`CaptureFrame::source_id` 仍标识所属来源。
单选也使用相同接口，不同来源的帧流可以移到不同任务，互不借用或消费彼此的帧。
帧序号和帧率限制分别按来源计算，多来源不保证帧同步，不进行图像拼接。

只有全部来源均启动后 `start()` 才成功；任一来源启动失败时清理整组。
`stop()`、丢弃会话或共享会话异常停止整组；来源消失或本路异常只结束该来源，
在该路交付一次终止错误；丢弃某路帧流只正常停止该来源。
其他来源继续捕获，全部来源停止后自动清理整组。尚未取出的来源仍然捕获，
但最多缓存一帧；若不需要它，可取出流后立即丢弃。来源快照保持固定，停止的
来源不能再次取流或重新启动。权限撤销、Portal 会话关闭和共享连接故障仍结束整组。

## 来源名称

`CaptureSource.title` 是平台提供的来源显示名称快照，不保证等同于窗口的真实标题。
名称在会话启动时确定，后续窗口重命名不更新这个固定快照。
Linux 的 Portal 标准响应没有保证返回标题；PipeWire 后端进一步读取已授权的
目标来源节点名称或描述。平台没有提供可用名称时保留空字符串，不生成
“窗口 1”等标签，也不把原生 ID 或应用自身的捕获 stream 名称当作来源标题。

## 恢复凭据与持久化

`SourceId` 仍然是后端实例内的临时标识，不能用来记住跨重启的来源。
`CaptureRestoreToken` 是字段私有、可通过 Serde 序列化的不透明数据，携带
后端标识、格式版本和负载；恢复时由后端校验。`Debug` 隐藏负载，序列化结果
包含真实凭据，不应写入日志。克隆或反序列化不会增加使用次数。

`CaptureTarget::RestoreOrPicker { token, kind }` 尝试恢复先前选择，无法恢复时
允许系统提示用户重新选择。两条路径都必须满足 `kind`，不承诺免交互；
恢复会创建新捕获会话，不会重新启动原会话。后端或格式不匹配、负载损坏返回
`InvalidRestoreToken`，不能通过自动打开选择器掩盖数据错误。
恢复凭据属于整组选源，一次多选仍只保存一个凭据。恢复时传入原先的来源过滤
和数量模式；若恢复返回多个来源但请求单选，后端报错，不能悄悄截断来源列表。

`CapturePersistence` 表达本次会话的请求范围：

- `None`：不申请持久化，是 `CaptureOptions::new` 的默认值。
- `Application`：仅在应用运行期间恢复，凭据保留在内存，不跨应用重启保存。
- `UntilRevoked`：请求跨应用重启恢复，直到授权撤销或凭据失效。

能力的 `restore_kinds` 表示恢复支持的来源过滤，`persistence_modes` 表示支持
请求的范围。不支持的请求分别返回 `Unsupported(Restore(kind))` 或
`Unsupported(Persistence(mode))`，不能静默忽略。支持请求不表示系统一定批准；
捕获成功但未获持久化授权时，`take_restore_token()` 返回 `None`。

以下辅助函数统一首次选源与后续恢复，返回值中的新凭据交给业务层立即保存：

```rust,no_run
use app_lib::platform::screencast::{
  CaptureError, CaptureMultiplicity, CaptureOptions, CapturePersistence, CaptureRestoreToken,
  CaptureSession, CaptureTarget, CursorMode, Screencast, SelectionKind,
};

async fn start_remembered(
  backend: &dyn Screencast,
  saved: Option<CaptureRestoreToken>,
  multiplicity: CaptureMultiplicity,
) -> Result<(Box<dyn CaptureSession>, Option<CaptureRestoreToken>), CaptureError> {
  let target = match saved {
    Some(token) => CaptureTarget::RestoreOrPicker {
      token,
      kind: SelectionKind::Window,
    },
    None => CaptureTarget::Picker(SelectionKind::Window),
  };
  let options = CaptureOptions::new(target, CursorMode::Hidden, Some(30))
    .with_multiplicity(multiplicity)
    .with_persistence(CapturePersistence::UntilRevoked);
  let mut session = backend.start(options).await?;
  let renewed = session.take_restore_token();
  Ok((session, renewed))
}
```

调用前据能力决定是否提供“记住选择”。首次使用选择器并申请持久化，成功后
立即保存新凭据，再取帧流；后续读取凭据并使用 `RestoreOrPicker`。凭据可通过
`serde_json::to_string(&token)` 保存，通过 `serde_json::from_str` 读取；实际文件、
数据库或其他存储由业务层选择，捕获后端不操作存储。

同一凭据的恢复操作必须串行，业务层应在提交请求前将旧凭据从可用存储中移除，
防止失败、取消或并发请求继续使用它。成功时保存返回的新凭据；返回 `None`
时保持无凭据状态。`take_restore_token()` 最多返回一次，不依赖首帧或停止，
停止后尚未取出的凭据仍可取出；停止捕获不等于撤销持久授权。

此接口不保证启动失败、取消、进程崩溃时新凭据可交付。例如 Linux Portal
`Start` 返回新凭据后，打开 remote 或 PipeWire 连接仍可能失败；旧凭据可能已经
失效，新的也可能未交付，下次需要重新授权。当前没有独立凭据更新或存储通道。

## 帧与生命周期契约

- 会话启动后可以暂时不取出或不轮询帧流，每路最多保留一个待消费图像；
  新帧仅覆盖该路旧帧，来源之间拥有独立缓存和唤醒器。
- 输出为自有 CPU 内存的 SDR sRGB RGBA8，非预乘 alpha，图像不依赖原生缓冲区存活。
- `timestamp` 相对同一个会话时钟，在每个来源内单调；不保证多来源同步。
  `sequence` 在每个来源内从零递增，丢帧可能产生空洞。
- `max_fps` 是每个来源的交付帧率上限，允许丢帧限速，不保证固定节拍或静止来源持续产帧。
- 尺寸可随帧变化；消费者从 `image` 读取实际像素宽高，不缓存初始尺寸作为永久值。
- 来源异常只清空并结束本路，交付一次终止错误，其他来源继续捕获。
  共享会话异常则在每个仍活动的来源流各交付一次终止错误后结束。
  正常停止清空待消费图像并结束流；已有终止错误不能被覆盖或被其他路取走。
- 停止后尚未取出的来源流仍可首次取出，保留未消费的终止错误，不保留图像。
- `stop` 幂等，释放不能依赖帧消费者继续运行；丢弃单路流只释放该来源，
  丢弃会话或全部来源停止才触发整组清理。
- 启动 future 被取消时清理部分资源并忽略迟到结果；`Drop` 仅保证请求清理，确认完成用 `stop`。

此契约聚焦 CPU 图像处理。多来源合成、音频、HDR 输出、GPU 零拷贝、凭据存储、
授权撤销操作、动态改选与重新配置不在当前接口范围内。

## 已有模型的调整

- `CaptureOptions::new` 保留原参数形式；`max_fps` 字段改为 `Option<NonZeroU32>`，
  原来的零值和省略值都归一化为 `None`。
- `CaptureFrame::timestamp` 从未定义单位的 `u128` 改为 `Duration`。
- `FrameStream` 从交付 `CaptureFrame` 改为交付 `ArcCaptureFrame`，便于多个图像处理任务共享帧。
- 补充来源 ID 的相等比较与哈希、能力快照和可区分的错误。
- 增加 `RestoreOrPicker`、恢复凭据、持久化请求、对应能力与错误；
  直接构造 `CaptureOptions` 或 `CaptureCapabilities` 时需补充新字段。
  通过 `CaptureOptions::new` 创建的请求默认不持久化，使用 `with_persistence` 显式申请。
- 增加来源数量模式和多选能力；会话的 `source()` 改为 `sources()`，
  `CaptureFrame` 增加 `source_id`，直接构造 `CaptureOptions` 时需补充 `multiplicity`。
- `take_frames()` 改为 `take_frames(&SourceId)`，每个来源可取出一次独立帧流。
  `CaptureError` 支持克隆，`Backend.source` 改为 `Arc<dyn Error + Send + Sync>`，
  保留原始错误链并支持整组异常向每路独立交付。

## 后续实现验收

各平台需分别验证权限允许/拒绝/撤销、用户取消、来源消失与尺寸变化、慢消费者、
启动取消、重复及并发停止、丢弃两种句柄后的资源释放。类型检查不能证明这些运行时契约。
恢复能力还需验证首次持久化、跨会话/重启恢复、凭据轮换、凭据失效后重新选源、
持久化未获批准，以及凭据已消耗后的启动失败和取消。
多选需验证多个窗口、多个显示器、混合来源、整组恢复、全部来源启动、任一来源
失效时整组终止、各来源独立消费/唤醒/帧率/缓存、单路流丢弃，以及整组资源释放。
公共接口不引入平台依赖。PipeWire 的纯 Rust 逻辑已有单元测试，
原生编译与桌面授权/采集的验证范围见后端说明。

## 官方参考

- [XDG Desktop Portal ScreenCast](https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.ScreenCast.html)
- [Windows 屏幕捕获](https://learn.microsoft.com/en-us/windows/apps/develop/media-authoring-processing/screen-capture)
- [IGraphicsCaptureItemInterop](https://learn.microsoft.com/en-us/windows/win32/api/windows.graphics.capture.interop/nn-windows-graphics-capture-interop-igraphicscaptureiteminterop)
- [SCShareableContent](https://developer.apple.com/documentation/screencapturekit/scshareablecontent)
- [SCContentSharingPicker](https://developer.apple.com/documentation/screencapturekit/sccontentsharingpicker)
