import { useCallback, useEffect, useRef, useState } from "react";
import {
  Alert,
  AppBar,
  Avatar,
  Box,
  Button,
  Card,
  Chip,
  Container,
  Dialog,
  DialogActions,
  DialogContent,
  DialogContentText,
  DialogTitle,
  Divider,
  IconButton,
  LinearProgress,
  List,
  ListItem,
  ListItemAvatar,
  ListItemButton,
  ListItemText,
  MenuItem,
  Stack,
  Switch,
  Tab,
  Tabs,
  TextField,
  ToggleButton,
  ToggleButtonGroup,
  Toolbar,
  Tooltip,
  Typography,
} from "@mui/material";
import { useColorScheme } from "@mui/material/styles";
import ShieldOutlinedIcon from "@mui/icons-material/ShieldOutlined";
import AddIcon from "@mui/icons-material/Add";
import SaveOutlinedIcon from "@mui/icons-material/SaveOutlined";
import PlayArrowIcon from "@mui/icons-material/PlayArrow";
import StopIcon from "@mui/icons-material/Stop";
import LightModeOutlinedIcon from "@mui/icons-material/LightModeOutlined";
import DarkModeOutlinedIcon from "@mui/icons-material/DarkModeOutlined";
import SettingsBrightnessOutlinedIcon from "@mui/icons-material/SettingsBrightnessOutlined";
import ContentCopyIcon from "@mui/icons-material/ContentCopy";
import DeleteOutlineIcon from "@mui/icons-material/DeleteOutlined";
import LinkOffIcon from "@mui/icons-material/LinkOff";
import CameraAltOutlinedIcon from "@mui/icons-material/CameraAltOutlined";
import DownloadOutlinedIcon from "@mui/icons-material/DownloadOutlined";
import RefreshIcon from "@mui/icons-material/Refresh";
import HistoryIcon from "@mui/icons-material/History";
import {
  copyCharacterRegions,
  invoke,
  newCharacter,
  regionNames,
} from "./reporting";
import type {
  Config,
  Region,
  RegionKey,
  ReportEvent,
  Snapshot,
  Status,
} from "./reporting";
import NotificationSettings from "./NotificationSettings";
import SnapshotEditor from "./SnapshotEditor";
import "./App.css";

export default function App() {
  const [config, setConfig] = useState<Config>({
    characters: [],
    report_methods: [],
  });
  const [path, setPath] = useState("");
  const [selected, setSelected] = useState(0);
  const [copyTarget, setCopyTarget] = useState<number | null>(null);
  const [regionKey, setRegionKey] = useState<RegionKey>("warn_region");
  const [snapshot, setSnapshot] = useState<Snapshot | null>(null);
  const [status, setStatus] = useState<Status>({ running: false, sources: [] });
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState("");
  const [error, setError] = useState("");
  const [history, setHistory] = useState<(ReportEvent & { time: string })[]>(
    [],
  );
  const sounds = useRef(new Map<string, HTMLAudioElement>());
  const character = config.characters[selected];
  const enabledCount = config.characters.filter((c) => c.enabled).length;
  const region = character?.[regionKey];
  const regionGroup = regionKey === "warn_region" ? "warn_region" : "reminder";
  const [loaded, setLoaded] = useState(false);
  const { mode, setMode } = useColorScheme();
  const desktop = Boolean(window.__TAURI__);
  const locked = busy || status.running || !loaded;
  const voiceEnabled = config.report_methods.some((m) => m.type === "Voice");

  const refreshStatus = useCallback(async () => {
    const result = await invoke<Status>("reporting_status");
    setStatus(result);
  }, []);
  async function action(work: () => Promise<void>) {
    setBusy(true);
    setError("");
    setMessage("");
    try {
      await work();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }
  useEffect(() => {
    if (!window.__TAURI__) {
      setLoaded(true);
      return;
    }
    void refreshStatus().catch((e: unknown) => setError(String(e)));
    void invoke<{ config: Config; path: string }>("reporting_load")
      .then((result) => {
        setConfig(result.config);
        setPath(result.path);
        setLoaded(true);
      })
      .catch((e: unknown) => setError(String(e)));
    const timer = window.setInterval(() => {
      void refreshStatus().catch((e: unknown) => setError(String(e)));
    }, 2000);
    return () => window.clearInterval(timer);
  }, [refreshStatus]);
  useEffect(() => {
    const api = window.__TAURI__;
    if (!api) return;
    let disposed = false;
    const unlisteners: (() => void)[] = [];
    const register = async () => {
      for (const promise of [
        api.event.listen<ReportEvent>("reporting-event", ({ payload }) => {
          setHistory((old) =>
            [
              { ...payload, time: new Date().toLocaleTimeString() },
              ...old,
            ].slice(0, 100),
          );
          const audioKind = payload.type === "Warn" ? "Warn" : "Reminder";
          if (!voiceEnabled || sounds.current.has(audioKind)) return;
          const audio = new Audio();
          sounds.current.set(audioKind, audio);
          const clear = () => sounds.current.delete(audioKind);
          audio.onended = clear;
          audio.onerror = () => {
            clear();
            setError("音频播放失败，请检查文件格式");
          };
          void invoke<string>("reporting_audio", { kind: payload.type })
            .then(async (url) => {
              if (sounds.current.get(audioKind) !== audio) return;
              audio.src = url;
              await audio.play();
            })
            .catch((e: unknown) => {
              clear();
              setError(String(e));
            });
        }),
        api.event.listen<string>("reporting-error", ({ payload }) =>
          setError(payload),
        ),
      ]) {
        try {
          const unlisten = await promise;
          if (disposed) unlisten();
          else unlisteners.push(unlisten);
        } catch (e) {
          setError(String(e));
        }
      }
    };
    void register();
    return () => {
      disposed = true;
      unlisteners.forEach((unlisten) => unlisten());
    };
  }, [voiceEnabled]);
  function updateRegion(value: Region) {
    setConfig((old) => ({
      ...old,
      characters: old.characters.map((c, i) =>
        i === selected ? { ...c, [regionKey]: value } : c,
      ),
    }));
  }
  async function save() {
    const saved = await invoke<string>("reporting_save", { config });
    setPath(saved);
    setMessage("配置已保存");
  }
  function addCharacter() {
    const existing = new Set(config.characters.map((c) => c.title));
    let index = 1;
    while (existing.has(`EVE - 角色${index}`)) index++;
    setConfig({
      ...config,
      characters: [...config.characters, newCharacter(index)],
    });
    setSelected(config.characters.length);
    setSnapshot(null);
  }
  const connected = Boolean(
    character && status.sources.includes(character.title),
  );

  return (
    <>
      <AppBar
        position="sticky"
        color="inherit"
        elevation={0}
        sx={{
          borderBottom: 1,
          borderColor: "divider",
          bgcolor: "background.paper",
        }}
      >
        <Toolbar sx={{ gap: 2, minHeight: "72px !important" }}>
          <Avatar
            variant="rounded"
            sx={{
              bgcolor: "primary.main",
              color: "primary.contrastText",
              width: 38,
              height: 38,
            }}
          >
            <ShieldOutlinedIcon />
          </Avatar>
          <Box sx={{ flex: 1 }}>
            <Typography sx={{ fontWeight: 750, letterSpacing: "-0.03em" }}>
              EVE Alert
            </Typography>
            <Typography variant="caption" color="text.secondary">
              画面监控与告警
            </Typography>
          </Box>
          <ToggleButtonGroup
            size="small"
            exclusive
            value={mode ?? "system"}
            onChange={(_, value) => {
              if (value) setMode(value);
            }}
            aria-label="界面主题"
          >
            <ToggleButton value="light" aria-label="浅色主题">
              <Tooltip title="浅色">
                <LightModeOutlinedIcon fontSize="small" />
              </Tooltip>
            </ToggleButton>
            <ToggleButton value="dark" aria-label="深色主题">
              <Tooltip title="深色">
                <DarkModeOutlinedIcon fontSize="small" />
              </Tooltip>
            </ToggleButton>
            <ToggleButton value="system" aria-label="跟随系统主题">
              <Tooltip title="跟随系统">
                <SettingsBrightnessOutlinedIcon fontSize="small" />
              </Tooltip>
            </ToggleButton>
          </ToggleButtonGroup>
        </Toolbar>
        {busy && (
          <LinearProgress
            sx={{ position: "absolute", left: 0, right: 0, bottom: 0 }}
          />
        )}
      </AppBar>
      <Container maxWidth="xl" component="main" sx={{ py: { xs: 2.5, md: 4 } }}>
        <Stack
          direction={{ xs: "column", md: "row" }}
          sx={{ justifyContent: "space-between", gap: 2, mb: 3 }}
        >
          <Box>
            <Typography
              variant="overline"
              color="primary"
              sx={{ letterSpacing: "0.14em" }}
            >
              MONITORING CONSOLE
            </Typography>
            <Typography variant="h4" component="h1" sx={{ mb: 0.75 }}>
              守望每一次画面变化
            </Typography>
            <Typography variant="body2" color="text.secondary">
              配置识别区域与目标颜色，让关键变化及时送达。
            </Typography>
          </Box>
          <Stack
            direction="row"
            useFlexGap
            sx={{ alignItems: "center", flexWrap: "wrap", gap: 1 }}
          >
            <Chip
              size="small"
              color={status.running ? "success" : "default"}
              variant="outlined"
              label={status.running ? "● 监控中" : "○ 未启动"}
            />
            <Button
              variant="outlined"
              startIcon={<SaveOutlinedIcon />}
              disabled={locked || !desktop}
              onClick={() => void action(save)}
            >
              保存配置
            </Button>
            <Button
              variant="contained"
              color={status.running ? "error" : "primary"}
              startIcon={status.running ? <StopIcon /> : <PlayArrowIcon />}
              disabled={
                busy ||
                !loaded ||
                (enabledCount === 0 && !status.running) ||
                !desktop
              }
              onClick={() =>
                void action(async () => {
                  if (status.running) {
                    await invoke("reporting_stop");
                    sounds.current.forEach((audio) => audio.pause());
                    sounds.current.clear();
                  } else {
                    await save();
                    await invoke("reporting_start", { config });
                  }
                  await refreshStatus();
                })
              }
            >
              {status.running ? "停止监控" : "开始监控"}
            </Button>
          </Stack>
        </Stack>
        {!desktop && (
          <Alert severity="info" sx={{ mb: 2 }}>
            当前为浏览器预览。来源授权、截图、配置存储和监控需在 Tauri
            桌面应用中使用。
          </Alert>
        )}
        {error && (
          <Alert
            severity="error"
            onClose={() => setError("")}
            sx={{ mb: 2, overflowWrap: "anywhere" }}
            action={
              !loaded ? (
                <Button
                  color="inherit"
                  onClick={() => window.location.reload()}
                >
                  重新加载
                </Button>
              ) : undefined
            }
          >
            {error}
          </Alert>
        )}
        {message && (
          <Alert
            severity="success"
            onClose={() => setMessage("")}
            sx={{ mb: 2, overflowWrap: "anywhere" }}
          >
            {message}
          </Alert>
        )}
        <Box className="workspace">
          <Card component="aside" sx={{ p: 2.5 }}>
            <Stack
              direction="row"
              sx={{
                justifyContent: "space-between",
                alignItems: "center",
                mb: 1,
              }}
            >
              <Typography variant="h6">
                监控角色{" "}
                <Typography
                  component="span"
                  variant="caption"
                  color="text.secondary"
                >
                  {enabledCount} / {config.characters.length} 已启用
                </Typography>
              </Typography>
              <Tooltip title="添加角色">
                <span>
                  <IconButton
                    size="small"
                    color="primary"
                    aria-label="添加角色"
                    disabled={locked}
                    onClick={addCharacter}
                  >
                    <AddIcon />
                  </IconButton>
                </span>
              </Tooltip>
            </Stack>
            {!config.characters.length && (
              <Box sx={{ py: 3 }}>
                <Typography
                  variant="body2"
                  color="text.secondary"
                  sx={{ mb: 2 }}
                >
                  添加第一个角色，为它选择要监控的游戏画面。
                </Typography>
                <Button
                  variant="outlined"
                  startIcon={<AddIcon />}
                  fullWidth
                  disabled={locked}
                  onClick={addCharacter}
                >
                  添加角色
                </Button>
              </Box>
            )}
            <List disablePadding aria-label="监控角色列表" sx={{ mb: 2 }}>
              {config.characters.map((c, i) => (
                <ListItem
                  key={i}
                  disablePadding
                  secondaryAction={
                    <Switch
                      size="small"
                      checked={c.enabled}
                      disabled={locked}
                      slotProps={{
                        input: { "aria-label": `启用角色 ${c.title}` },
                      }}
                      onChange={(_, enabled) =>
                        setConfig((old) => ({
                          ...old,
                          characters: old.characters.map((item, index) =>
                            index === i ? { ...item, enabled } : item,
                          ),
                        }))
                      }
                    />
                  }
                >
                  <ListItemButton
                    selected={selected === i}
                    disabled={busy}
                    onClick={() => {
                      if (selected !== i) {
                        setSelected(i);
                        setSnapshot(null);
                      }
                    }}
                    sx={{ borderRadius: 2, pl: 1, pr: 7, my: 0.5 }}
                  >
                    <ListItemAvatar sx={{ minWidth: 44 }}>
                      <Avatar
                        sx={{
                          width: 32,
                          height: 32,
                          fontSize: 13,
                          bgcolor:
                            selected === i ? "primary.main" : "action.selected",
                          color:
                            selected === i
                              ? "primary.contrastText"
                              : "text.secondary",
                        }}
                      >
                        {String(i + 1).padStart(2, "0")}
                      </Avatar>
                    </ListItemAvatar>
                    <ListItemText
                      primary={c.title || "未命名角色"}
                      secondary={
                        !c.enabled
                          ? "已禁用"
                          : status.sources.includes(c.title)
                            ? "● 来源已连接"
                            : "等待选择来源"
                      }
                      slotProps={{
                        primary: {
                          variant: "body2",
                          sx: { fontWeight: 600, overflowWrap: "anywhere" },
                        },
                        secondary: {
                          variant: "caption",
                          sx: {
                            color:
                              c.enabled && status.sources.includes(c.title)
                                ? "success.main"
                                : "text.secondary",
                          },
                        },
                      }}
                    />
                  </ListItemButton>
                </ListItem>
              ))}
            </List>
            {character && (
              <>
                <Divider sx={{ mb: 2.5 }} />
                <TextField
                  label="角色 / 窗口名称"
                  disabled={locked || connected}
                  value={character.title}
                  onChange={(e) => {
                    setConfig({
                      ...config,
                      characters: config.characters.map((c, i) =>
                        i === selected ? { ...c, title: e.target.value } : c,
                      ),
                    });
                    setSnapshot(null);
                  }}
                  helperText={
                    connected
                      ? "先释放来源，再修改名称"
                      : "用于事件标识，请在系统选择器中选择对应画面"
                  }
                />
                <Stack sx={{ gap: 0.5, mt: 2 }}>
                  <Button
                    startIcon={<ContentCopyIcon />}
                    disabled={locked}
                    sx={{ justifyContent: "flex-start" }}
                    onClick={() => {
                      const copy = structuredClone(character);
                      const existing = new Set(
                        config.characters.map((c) => c.title),
                      );
                      const base = character.title.trim() || "EVE - 角色";
                      let index = 1;
                      do {
                        copy.title = `${base}（副本${index === 1 ? "" : index}）`;
                        index++;
                      } while (existing.has(copy.title));
                      setConfig({
                        ...config,
                        characters: [...config.characters, copy],
                      });
                      setSelected(config.characters.length);
                      setSnapshot(null);
                      setError("");
                      setMessage(
                        "角色配置已复制，请为新角色选择捕获来源，完成后保存配置。",
                      );
                    }}
                  >
                    复制为新角色
                  </Button>
                  <Button
                    startIcon={<ContentCopyIcon />}
                    disabled={locked || config.characters.length < 2}
                    sx={{ justifyContent: "flex-start" }}
                    onClick={() =>
                      setCopyTarget(
                        config.characters.findIndex((_, i) => i !== selected),
                      )
                    }
                  >
                    覆盖到其他角色
                  </Button>
                  <Button
                    startIcon={<LinkOffIcon />}
                    disabled={locked || !connected || !desktop}
                    sx={{ justifyContent: "flex-start" }}
                    onClick={() =>
                      void action(async () => {
                        await invoke("reporting_release", {
                          title: character.title,
                        });
                        setSnapshot(null);
                        await refreshStatus();
                      })
                    }
                  >
                    释放来源
                  </Button>
                  <Button
                    color="error"
                    startIcon={<DeleteOutlineIcon />}
                    disabled={locked}
                    sx={{ justifyContent: "flex-start" }}
                    onClick={() =>
                      void action(async () => {
                        if (desktop)
                          await invoke("reporting_release", {
                            title: character.title,
                          });
                        setConfig({
                          ...config,
                          characters: config.characters.filter(
                            (_, i) => i !== selected,
                          ),
                        });
                        setSelected(0);
                        setSnapshot(null);
                        if (desktop) await refreshStatus();
                      })
                    }
                  >
                    移除角色
                  </Button>
                </Stack>
              </>
            )}
            <Box
              sx={{ mt: 3, p: 1.5, borderRadius: 2, bgcolor: "action.hover" }}
            >
              <Typography variant="caption" color="text.secondary">
                每个角色独立识别 · 最高 2 FPS
                <br />
                停止监控后保留来源恢复凭据
                <br />
                停止监控后可切换角色启用状态
              </Typography>
            </Box>
          </Card>
          <Card component="section" sx={{ minWidth: 0, overflow: "hidden" }}>
            <Box sx={{ p: 2.5, pb: 2 }}>
              <Stack
                direction="row"
                sx={{
                  justifyContent: "space-between",
                  alignItems: "center",
                  mb: 1,
                  gap: 1,
                }}
              >
                <Typography variant="h6">识别区域</Typography>
                {character && (
                  <Chip
                    size="small"
                    color={connected ? "success" : "default"}
                    variant="outlined"
                    label={connected ? "来源已连接" : "尚未连接来源"}
                  />
                )}
              </Stack>
              <Typography variant="body2" color="text.secondary" sx={{ mb: 2 }}>
                选择画面 → 框选区域 → 采集颜色
              </Typography>
              <Stack
                direction="row"
                useFlexGap
                sx={{ gap: 1, flexWrap: "wrap" }}
              >
                <Button
                  variant={connected ? "outlined" : "contained"}
                  startIcon={<CameraAltOutlinedIcon />}
                  disabled={locked || !character || !desktop}
                  onClick={() =>
                    void action(async () => {
                      await invoke("reporting_select", {
                        title: character.title,
                      });
                      setSnapshot(null);
                      await refreshStatus();
                      setMessage("来源已授权；收到首帧后点击刷新截图");
                    })
                  }
                >
                  获取 / 恢复来源
                </Button>
                <Button
                  disabled={locked || !character || !desktop}
                  onClick={() =>
                    void action(async () => {
                      await invoke("reporting_select", {
                        title: character.title,
                        reselect: true,
                      });
                      setSnapshot(null);
                      await refreshStatus();
                      setMessage("来源已重新获取；收到首帧后点击刷新截图");
                    })
                  }
                >
                  重新获取来源
                </Button>
                <Button
                  startIcon={<RefreshIcon />}
                  disabled={busy || !connected || !desktop}
                  onClick={() =>
                    void action(async () =>
                      setSnapshot(
                        await invoke<Snapshot>("reporting_snapshot", {
                          title: character.title,
                        }),
                      ),
                    )
                  }
                >
                  刷新截图
                </Button>
                <Button
                  startIcon={<DownloadOutlinedIcon />}
                  disabled={busy || !connected || !desktop}
                  onClick={() =>
                    void action(async () => {
                      const saved = await invoke<string>("reporting_export", {
                        title: character.title,
                      });
                      setMessage(`截图已保存：${saved}`);
                    })
                  }
                >
                  导出截图
                </Button>
              </Stack>
            </Box>
            <Tabs
              value={regionGroup}
              onChange={(_, value: "warn_region" | "reminder") =>
                setRegionKey(
                  value === "reminder" ? "reminder_now_region" : value,
                )
              }
              variant="scrollable"
              scrollButtons="auto"
              aria-label="识别区域类型"
              sx={{ px: 2, borderBottom: 1, borderColor: "divider" }}
            >
              <Tab
                id="region-tab-warn_region"
                aria-controls="region-panel-warn_region"
                value="warn_region"
                disabled={busy}
                label="01  告警区域"
              />
              <Tab
                id="region-tab-reminder"
                aria-controls="region-panel-reminder"
                value="reminder"
                disabled={busy}
                label="02  提醒配置"
              />
            </Tabs>
            <Box
              role="tabpanel"
              id={`region-panel-${regionGroup}`}
              aria-labelledby={`region-tab-${regionGroup}`}
              sx={{ p: 2.5 }}
            >
              {character && region ? (
                <>
                  <SnapshotEditor
                    key={`${selected}-${regionGroup}`}
                    snapshot={snapshot}
                    region={region}
                    locked={locked}
                    onChange={updateRegion}
                    regionKey={regionKey}
                    regions={
                      regionGroup === "reminder"
                        ? [
                            {
                              key: "reminder_now_region",
                              label: regionNames.reminder_now_region,
                              region: character.reminder_now_region,
                              color: "#6554c0",
                            },
                            {
                              key: "reminder_gate_region",
                              label: regionNames.reminder_gate_region,
                              region: character.reminder_gate_region,
                              color: "#c77800",
                            },
                            {
                              key: "reminder_enemy_region",
                              label: regionNames.reminder_enemy_region,
                              region: character.reminder_enemy_region,
                              color: "#00897b",
                            },
                          ]
                        : undefined
                    }
                    onSelectRegion={setRegionKey}
                  />
                  <Alert
                    severity="info"
                    icon={false}
                    sx={{ mt: 2, fontSize: 12 }}
                  >
                    {regionKey === "warn_region"
                      ? "告警：目标色首次出现时通知，持续命中不重复；全部消失后发送一次解除。"
                      : "区域 1 出现 RGB1 时提醒“异常进入最后一波次”，开启区域 2 检测；区域 2 命中任意一种 RGB2 后开启区域 3 检测，RGB2 后续消失不影响检测。区域 3 记录到任意一种 RGB3 后，所有 RGB3 都消失才提醒“异常结束”。区域 1 的 RGB1 消失也会结束本轮。结束后等待 RGB1 消失并重新出现，才开始下一轮。"}
                  </Alert>
                </>
              ) : (
                <Stack
                  sx={{
                    py: 9,
                    alignItems: "center",
                    textAlign: "center",
                    gap: 1.5,
                  }}
                >
                  <CropPlaceholder />
                  <Typography variant="h6">先添加一个监控角色</Typography>
                  <Typography variant="body2" color="text.secondary">
                    每个角色可配置告警区域，并在提醒配置中分别设置三个区域与
                    RGB。
                  </Typography>
                  <Button
                    startIcon={<AddIcon />}
                    disabled={locked}
                    onClick={addCharacter}
                  >
                    添加角色
                  </Button>
                </Stack>
              )}
            </Box>
          </Card>
        </Box>
        <Card component="section" sx={{ p: 2.5, mt: 3 }}>
          <NotificationSettings
            methods={config.report_methods}
            locked={locked}
            onChange={(methods) =>
              setConfig({ ...config, report_methods: methods })
            }
            onError={setError}
          />
        </Card>
        <Card component="section" sx={{ p: 2.5, mt: 3 }}>
          <Stack
            direction="row"
            sx={{
              alignItems: "center",
              justifyContent: "space-between",
              mb: 1,
            }}
          >
            <Typography variant="h6">
              事件记录{" "}
              <Typography
                component="span"
                variant="caption"
                color="text.secondary"
              >
                最近 100 条
              </Typography>
            </Typography>
            <Button disabled={!history.length} onClick={() => setHistory([])}>
              清空记录
            </Button>
          </Stack>
          {history.length ? (
            <Box
              sx={{ maxHeight: 280, overflowY: "auto" }}
              aria-live="polite"
              role="log"
            >
              {history.map((event, i) => (
                <Stack
                  key={i}
                  direction="row"
                  sx={{
                    alignItems: "center",
                    gap: 2,
                    py: 1.5,
                    borderBottom: 1,
                    borderColor: "divider",
                  }}
                >
                  <Typography
                    component="time"
                    variant="caption"
                    color="text.secondary"
                    sx={{ fontVariantNumeric: "tabular-nums" }}
                  >
                    {event.time}
                  </Typography>
                  <Chip
                    size="small"
                    color={
                      event.type === "Warn"
                        ? "error"
                        : event.type === "WarnCleared"
                          ? "success"
                          : "info"
                    }
                    label={
                      {
                        Warn: "告警",
                        WarnCleared: "告警解除",
                        Reminder: "提醒",
                      }[event.type]
                    }
                  />
                  <Typography variant="body2" sx={{ overflowWrap: "anywhere" }}>
                    {event.title}
                    {event.type === "Reminder" && `：${event.message}`}
                  </Typography>
                </Stack>
              ))}
            </Box>
          ) : (
            <Stack direction="row" sx={{ alignItems: "center", gap: 2, py: 3 }}>
              <HistoryIcon color="disabled" />
              <Typography variant="body2" color="text.secondary">
                暂无事件。开始监控后，告警与提醒会记录在这里。
              </Typography>
            </Stack>
          )}
        </Card>
        <Typography
          component="footer"
          variant="caption"
          color="text.secondary"
          sx={{ mt: 2.5, overflowWrap: "anywhere" }}
        >
          配置文件：{path || "尚未读取"} · 更换窗口或屏幕时，请重新获取来源。
        </Typography>
      </Container>
      <Dialog
        open={copyTarget !== null}
        onClose={() => setCopyTarget(null)}
        fullWidth
        maxWidth="sm"
        aria-labelledby="copy-character-title"
      >
        <DialogTitle id="copy-character-title">
          覆盖其他角色的识别配置
        </DialogTitle>
        <DialogContent>
          <DialogContentText sx={{ mb: 2 }}>
            将“{character?.title}”的告警区域、三个提醒区域及各自 RGB
            覆盖到目标角色。
            目标角色的名称、来源授权和启用状态保留。完成后请保存配置。
          </DialogContentText>
          <TextField
            select
            fullWidth
            label="目标角色"
            value={copyTarget ?? ""}
            disabled={locked}
            onChange={(event) => setCopyTarget(Number(event.target.value))}
          >
            {config.characters.map((item, i) =>
              i === selected ? null : (
                <MenuItem key={i} value={i}>
                  {item.title || `未命名角色 ${i + 1}`}
                  {!item.enabled && "（已禁用）"}
                </MenuItem>
              ),
            )}
          </TextField>
        </DialogContent>
        <DialogActions>
          <Button onClick={() => setCopyTarget(null)}>取消</Button>
          <Button
            variant="contained"
            disabled={
              locked ||
              copyTarget === null ||
              copyTarget === selected ||
              !config.characters[copyTarget]
            }
            onClick={() => {
              if (copyTarget === null || copyTarget === selected || !character)
                return;
              const target = config.characters[copyTarget];
              if (!target) return;
              setConfig((old) => ({
                ...old,
                characters: old.characters.map((item, i) =>
                  i === copyTarget
                    ? copyCharacterRegions(old.characters[selected], item)
                    : item,
                ),
              }));
              setCopyTarget(null);
              setError("");
              setMessage(
                `已用“${character.title}”的识别配置覆盖“${target.title}”，请保存配置。`,
              );
            }}
          >
            确认覆盖
          </Button>
        </DialogActions>
      </Dialog>
    </>
  );
}

function CropPlaceholder() {
  return (
    <CameraAltOutlinedIcon sx={{ fontSize: 48, color: "text.disabled" }} />
  );
}
