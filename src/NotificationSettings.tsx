import { useState } from "react";
import {
  Accordion,
  AccordionDetails,
  AccordionSummary,
  Box,
  Button,
  Chip,
  FormControlLabel,
  IconButton,
  Menu,
  MenuItem,
  Stack,
  Switch,
  TextField,
  Typography,
} from "@mui/material";
import AddIcon from "@mui/icons-material/Add";
import DeleteOutlineIcon from "@mui/icons-material/DeleteOutlined";
import ExpandMoreIcon from "@mui/icons-material/ExpandMore";
import NotificationsNoneIcon from "@mui/icons-material/NotificationsNone";
import type { Method } from "./reporting";

const methodNames = {
  Notification: "桌面通知",
  Ntfy: "ntfy 推送",
  Voice: "声音播放",
  Sse: "SSE 服务",
  ReverseWebsocket: "反向 WebSocket",
  Onebot: "OneBot 私聊",
};

function newMethod(type: Method["type"]): Method {
  switch (type) {
    case "Notification":
      return { type };
    case "Voice":
      return { type, warn_voice_path: "", reminder_voice_path: "" };
    case "Sse":
      return { type, host: [127, 0, 0, 1], port: 8080 };
    case "Onebot":
      return { type, url: "ws://127.0.0.1:3001/api", token: "", target: 0 };
    case "Ntfy":
      return { type, server_url: "https://ntfy.sh", topic: "", token: "" };
    case "ReverseWebsocket":
      return {
        type,
        url: "ws://127.0.0.1:8080/ws",
        try_forever: true,
        try_spacing: { secs: 5, nanos: 0 },
      };
  }
}

type Props = {
  methods: Method[];
  locked: boolean;
  onChange: (methods: Method[]) => void;
  onError: (error: string) => void;
};

export default function NotificationSettings({
  methods,
  locked,
  onChange,
  onError,
}: Props) {
  const [anchor, setAnchor] = useState<HTMLElement | null>(null);
  function update(index: number, method: Method) {
    onChange(methods.map((value, i) => (i === index ? method : value)));
  }
  return (
    <>
      <Stack
        direction="row"
        sx={{
          alignItems: "center",
          justifyContent: "space-between",
          gap: 2,
          mb: 2,
        }}
      >
        <Box>
          <Typography variant="h6">
            通知渠道 <Chip size="small" label={methods.length} sx={{ ml: 1 }} />
          </Typography>
          <Typography variant="body2" color="text.secondary">
            把告警、解除与提醒送达你关注的地方。
          </Typography>
        </Box>
        <Button
          variant="outlined"
          startIcon={<AddIcon />}
          disabled={locked}
          onClick={(e) => setAnchor(e.currentTarget)}
          aria-haspopup="menu"
          aria-expanded={Boolean(anchor)}
        >
          添加渠道
        </Button>
      </Stack>
      <Menu
        anchorEl={anchor}
        open={Boolean(anchor)}
        onClose={() => setAnchor(null)}
      >
        {(Object.keys(methodNames) as Method["type"][]).map((type) => (
          <MenuItem
            key={type}
            disabled={
              locked ||
              ((type === "Voice" || type === "Notification") &&
                methods.some((m) => m.type === type))
            }
            onClick={() => {
              onChange([...methods, newMethod(type)]);
              setAnchor(null);
            }}
          >
            {methodNames[type]}
          </MenuItem>
        ))}
      </Menu>
      {!methods.length && (
        <Stack
          direction="row"
          sx={{
            gap: 2,
            alignItems: "center",
            p: 2.5,
            bgcolor: "action.hover",
            borderRadius: 2,
          }}
        >
          <NotificationsNoneIcon color="disabled" />
          <Typography variant="body2" color="text.secondary">
            尚未添加通知渠道。监控事件仍会显示在事件记录中。
          </Typography>
        </Stack>
      )}
      {methods.map((method, index) => (
        <Stack
          direction="row"
          key={`${method.type}-${index}`}
          sx={{ alignItems: "flex-start", gap: 0.5, mb: 1 }}
        >
          <Accordion
            defaultExpanded
            disableGutters
            sx={{
              flex: 1,
              minWidth: 0,
              border: 1,
              borderColor: "divider",
              borderRadius: "10px !important",
              "&:before": { display: "none" },
            }}
          >
            <AccordionSummary expandIcon={<ExpandMoreIcon />}>
              <Typography variant="subtitle2">
                {methodNames[method.type]}
              </Typography>
            </AccordionSummary>
            <AccordionDetails>
              <Box
                component="fieldset"
                disabled={locked}
                className="method-fields"
              >
                {method.type === "Notification" && (
                  <Typography variant="body2" color="text.secondary">
                    通过 Linux 桌面通知服务显示告警、告警解除与提醒。
                  </Typography>
                )}
                {method.type === "Ntfy" && (
                  <>
                    <TextField
                      disabled={locked}
                      label="服务器地址"
                      type="url"
                      placeholder="https://ntfy.sh"
                      value={method.server_url}
                      onChange={(e) =>
                        update(index, { ...method, server_url: e.target.value })
                      }
                    />
                    <TextField
                      disabled={locked}
                      label="主题（Topic）"
                      value={method.topic}
                      onChange={(e) =>
                        update(index, { ...method, topic: e.target.value })
                      }
                    />
                    <TextField
                      disabled={locked}
                      label="访问令牌（可空）"
                      type="password"
                      autoComplete="off"
                      value={method.token}
                      onChange={(e) =>
                        update(index, { ...method, token: e.target.value })
                      }
                    />
                    <Typography
                      className="field-help"
                      variant="caption"
                      color="text.secondary"
                    >
                      在 ntfy
                      客户端订阅相同服务器与主题。告警首次出现与解除各推送一次，提醒单独推送。
                    </Typography>
                  </>
                )}
                {method.type === "Voice" && (
                  <>
                    {(["warn_voice_path", "reminder_voice_path"] as const).map(
                      (key) => (
                        <TextField
                          disabled={locked}
                          key={key}
                          label={
                            key === "warn_voice_path"
                              ? "告警音频路径"
                              : "提醒 / 告警解除音频路径"
                          }
                          placeholder="本机音频文件的绝对路径"
                          value={method[key]}
                          onChange={(e) =>
                            update(index, { ...method, [key]: e.target.value })
                          }
                        />
                      ),
                    )}
                    <Typography
                      className="field-help"
                      variant="caption"
                      color="text.secondary"
                    >
                      由桌面 WebView 播放，同类音频播放期间不叠加。
                    </Typography>
                  </>
                )}
                {method.type === "Sse" && (
                  <>
                    <TextField
                      disabled={locked}
                      key={method.host.join(".")}
                      label="监听 IPv4"
                      defaultValue={method.host.join(".")}
                      onBlur={(e) => {
                        const parts = e.target.value.split(".");
                        if (
                          parts.length === 4 &&
                          parts.every(
                            (n) => /^\d{1,3}$/.test(n) && Number(n) <= 255,
                          )
                        )
                          update(index, {
                            ...method,
                            host: parts.map(Number) as [
                              number,
                              number,
                              number,
                              number,
                            ],
                          });
                        else {
                          e.target.value = method.host.join(".");
                          onError("请输入有效的 IPv4 地址");
                        }
                      }}
                    />
                    <TextField
                      disabled={locked}
                      label="端口"
                      type="number"
                      slotProps={{ htmlInput: { min: 1, max: 65535 } }}
                      value={method.port}
                      onChange={(e) =>
                        update(index, {
                          ...method,
                          port: Number(e.target.value),
                        })
                      }
                    />
                    <Typography
                      className="field-help"
                      variant="caption"
                      color="text.secondary"
                    >
                      GET /events；首条为角色名称列表，后续为 Warn / WarnCleared
                      / Reminder 事件。
                    </Typography>
                  </>
                )}
                {method.type === "ReverseWebsocket" && (
                  <>
                    <TextField
                      disabled={locked}
                      label="服务地址"
                      placeholder="ws:// 或 wss://"
                      value={method.url}
                      onChange={(e) =>
                        update(index, { ...method, url: e.target.value })
                      }
                    />
                    <TextField
                      disabled={locked}
                      label="重试间隔（秒）"
                      type="number"
                      slotProps={{ htmlInput: { min: 1 } }}
                      value={method.try_spacing.secs}
                      onChange={(e) =>
                        update(index, {
                          ...method,
                          try_spacing: {
                            secs: Number(e.target.value),
                            nanos: 0,
                          },
                        })
                      }
                    />
                    <FormControlLabel
                      control={
                        <Switch
                          checked={method.try_forever}
                          disabled={locked}
                          onChange={(e) =>
                            update(index, {
                              ...method,
                              try_forever: e.target.checked,
                            })
                          }
                        />
                      }
                      label="断线持续重连"
                    />
                  </>
                )}
                {method.type === "Onebot" && (
                  <>
                    <TextField
                      disabled={locked}
                      label="OneBot WebSocket 地址"
                      value={method.url}
                      onChange={(e) =>
                        update(index, { ...method, url: e.target.value })
                      }
                    />
                    <TextField
                      disabled={locked}
                      label="访问令牌（可空）"
                      type="password"
                      autoComplete="off"
                      value={method.token}
                      onChange={(e) =>
                        update(index, { ...method, token: e.target.value })
                      }
                    />
                    <TextField
                      disabled={locked}
                      label="私聊目标 ID"
                      type="number"
                      slotProps={{ htmlInput: { min: 1, step: 1 } }}
                      value={method.target}
                      onChange={(e) =>
                        update(index, {
                          ...method,
                          target: Number(e.target.value),
                        })
                      }
                    />
                    <Typography
                      className="field-help"
                      variant="caption"
                      color="text.secondary"
                    >
                      直接连接 OneBot 11；告警首次出现与解除各发送一次，断线后每
                      5 秒重连。
                    </Typography>
                  </>
                )}
              </Box>
            </AccordionDetails>
          </Accordion>
          <IconButton
            aria-label={`移除${methodNames[method.type]}`}
            disabled={locked}
            onClick={() => onChange(methods.filter((_, i) => i !== index))}
            sx={{ mt: 1 }}
          >
            <DeleteOutlineIcon fontSize="small" />
          </IconButton>
        </Stack>
      ))}
    </>
  );
}
