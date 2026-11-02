import { useEffect, useLayoutEffect, useRef, useState } from "react";
import type { PointerEvent, KeyboardEvent } from "react";
import {
  Alert,
  Box,
  Button,
  Chip,
  Dialog,
  Divider,
  IconButton,
  Stack,
  TextField,
  ToggleButton,
  ToggleButtonGroup,
  Tooltip,
  Typography,
} from "@mui/material";
import CropFreeIcon from "@mui/icons-material/CropFree";
import ColorizeIcon from "@mui/icons-material/Colorize";
import PanToolOutlinedIcon from "@mui/icons-material/PanToolOutlined";
import ZoomInIcon from "@mui/icons-material/ZoomIn";
import ZoomOutIcon from "@mui/icons-material/ZoomOut";
import FullscreenIcon from "@mui/icons-material/Fullscreen";
import CloseIcon from "@mui/icons-material/Close";
import AddIcon from "@mui/icons-material/Add";
import ImageSearchOutlinedIcon from "@mui/icons-material/ImageSearchOutlined";
import { colorHex, hexRgb } from "./reporting";
import type { Region, Snapshot } from "./reporting";
import {
  clamp,
  drawnRegion,
  handlePosition,
  handles,
  movedRegion,
  pixelPoint,
  resizedRegion,
  validRegion,
} from "./selection";
import type { Handle, Point } from "./selection";

type Props = {
  snapshot: Snapshot | null;
  region: Region;
  locked: boolean;
  onChange: (region: Region) => void;
};
type Drag = {
  pointerId: number;
  origin: Point;
  original: Region;
  kind: "draw" | "move" | "pan" | Handle;
  client: Point;
  scroll: Point;
};

export default function SnapshotEditor({
  snapshot,
  region,
  locked,
  onChange,
}: Props) {
  const [mode, setMode] = useState<"region" | "color" | "pan">("region");
  const [zoom, setZoom] = useState<number | null>(null);
  const [expanded, setExpanded] = useState(false);
  const [draft, setDraft] = useState<Region | null>(null);
  const [hover, setHover] = useState<{
    point: Point;
    rgb: [number, number, number];
  } | null>(null);
  const [imageReady, setImageReady] = useState(false);
  const [imageError, setImageError] = useState("");
  const [scale, setScale] = useState(1);
  const viewport = useRef<HTMLDivElement>(null);
  const stage = useRef<HTMLDivElement>(null);
  const original = useRef<HTMLCanvasElement | null>(null);
  const magnifier = useRef<HTMLCanvasElement>(null);
  const drag = useRef<Drag | null>(null);
  const anchor = useRef<Point | null>(null);
  const current = draft ?? region;
  const valid = snapshot
    ? validRegion(current, snapshot.width, snapshot.height)
    : true;

  useEffect(() => {
    setImageReady(false);
    setImageError("");
    setHover(null);
    original.current = null;
    if (!snapshot) return;
    let disposed = false;
    const image = new Image();
    image.onload = () => {
      if (disposed) return;
      const canvas = document.createElement("canvas");
      canvas.width = snapshot.width;
      canvas.height = snapshot.height;
      const ctx = canvas.getContext("2d", { willReadFrequently: true });
      if (!ctx) {
        setImageError("无法创建像素取色画布");
        return;
      }
      ctx.drawImage(image, 0, 0);
      original.current = canvas;
      setImageReady(true);
    };
    image.onerror = () => {
      if (!disposed) setImageError("截图加载失败，请刷新截图");
    };
    image.src = snapshot.data_url;
    return () => {
      disposed = true;
    };
  }, [snapshot]);

  useEffect(() => {
    const element = stage.current;
    if (!element || !snapshot) return;
    const observer = new ResizeObserver(() =>
      setScale(element.clientWidth / snapshot.width),
    );
    observer.observe(element);
    return () => observer.disconnect();
  }, [snapshot, expanded]);

  useLayoutEffect(() => {
    if (!anchor.current || !stage.current || !viewport.current) return;
    const view = viewport.current;
    view.scrollLeft =
      anchor.current[0] * stage.current.clientWidth - view.clientWidth / 2;
    view.scrollTop =
      anchor.current[1] * stage.current.clientHeight - view.clientHeight / 2;
    anchor.current = null;
  }, [zoom]);

  useEffect(() => {
    const ctx = magnifier.current?.getContext("2d");
    if (!ctx || !original.current || !hover) return;
    ctx.imageSmoothingEnabled = false;
    ctx.fillStyle = "#555";
    ctx.fillRect(0, 0, 99, 99);
    ctx.drawImage(
      original.current,
      hover.point[0] - 5,
      hover.point[1] - 5,
      11,
      11,
      0,
      0,
      99,
      99,
    );
    ctx.strokeStyle = "#fff";
    ctx.lineWidth = 1;
    ctx.strokeRect(45.5, 45.5, 8, 8);
    ctx.strokeStyle = "#111";
    ctx.strokeRect(44.5, 44.5, 10, 10);
  }, [hover, expanded]);

  function changeZoom(value: number | null) {
    if (drag.current) return;
    const view = viewport.current;
    const image = stage.current;
    if (view && image)
      anchor.current = [
        (view.scrollLeft + view.clientWidth / 2) / image.clientWidth,
        (view.scrollTop + view.clientHeight / 2) / image.clientHeight,
      ];
    setZoom(value === null ? null : clamp(value, 0.125, 16));
  }

  function point(event: PointerEvent<HTMLDivElement>): Point {
    return pixelPoint(
      event.clientX,
      event.clientY,
      stage.current!.getBoundingClientRect(),
      snapshot!.width,
      snapshot!.height,
    );
  }

  function sample(p: Point) {
    const data = original.current
      ?.getContext("2d")
      ?.getImageData(p[0], p[1], 1, 1).data;
    if (!data) return null;
    const rgb: [number, number, number] = [data[0], data[1], data[2]];
    setHover({ point: p, rgb });
    return rgb;
  }

  function regionAt(p: Point, active: Drag) {
    if (!snapshot) return active.original;
    if (active.kind === "draw")
      return drawnRegion(active.original, active.origin, p);
    if (active.kind === "move")
      return movedRegion(
        active.original,
        [p[0] - active.origin[0], p[1] - active.origin[1]],
        snapshot.width,
        snapshot.height,
      );
    if (active.kind === "pan") return active.original;
    return resizedRegion(
      active.original,
      active.kind,
      [p[0] - active.origin[0], p[1] - active.origin[1]],
      snapshot.width,
      snapshot.height,
    );
  }

  function pointerDown(event: PointerEvent<HTMLDivElement>) {
    if (
      !snapshot ||
      !imageReady ||
      (event.button !== 0 && event.button !== 1) ||
      drag.current
    )
      return;
    event.preventDefault();
    event.currentTarget.focus({ preventScroll: true });
    const p = point(event);
    const pan = mode === "pan" || event.button === 1;
    if (locked && !pan) return;
    if (mode === "color" && !pan) {
      const rgb = sample(p);
      if (rgb && !region.rgb.some((color) => colorHex(color) === colorHex(rgb)))
        onChange({ ...region, rgb: [...region.rgb, rgb] });
      return;
    }
    const target = event.target as HTMLElement;
    const kind = pan
      ? "pan"
      : event.shiftKey
        ? "draw"
        : ((target.dataset.handle as Handle | undefined) ??
          (target.dataset.move && valid ? "move" : "draw"));
    drag.current = {
      pointerId: event.pointerId,
      origin: p,
      original: region,
      kind,
      client: [event.clientX, event.clientY],
      scroll: [viewport.current!.scrollLeft, viewport.current!.scrollTop],
    };
    if (kind === "draw") setDraft(drawnRegion(region, p, p));
    event.currentTarget.setPointerCapture(event.pointerId);
  }

  function pointerMove(event: PointerEvent<HTMLDivElement>) {
    if (!snapshot || !imageReady) return;
    const active = drag.current;
    if (active && active.pointerId !== event.pointerId) return;
    if (active?.kind === "pan") {
      viewport.current!.scrollLeft =
        active.scroll[0] - (event.clientX - active.client[0]);
      viewport.current!.scrollTop =
        active.scroll[1] - (event.clientY - active.client[1]);
      return;
    }
    const p = point(event);
    sample(p);
    if (active) setDraft(regionAt(p, active));
  }

  function finish(event: PointerEvent<HTMLDivElement>, cancel = false) {
    const active = drag.current;
    if (!active || active.pointerId !== event.pointerId) return;
    if (!cancel && !locked && active.kind !== "pan")
      onChange(regionAt(point(event), active));
    drag.current = null;
    setDraft(null);
    if (event.currentTarget.hasPointerCapture(event.pointerId))
      event.currentTarget.releasePointerCapture(event.pointerId);
  }

  function keyDown(event: KeyboardEvent<HTMLDivElement>) {
    if (event.key === "Escape") {
      if (drag.current) {
        drag.current = null;
        setDraft(null);
      } else setExpanded(false);
      event.stopPropagation();
      return;
    }
    const directions: Record<string, Point> = {
      ArrowLeft: [-1, 0],
      ArrowRight: [1, 0],
      ArrowUp: [0, -1],
      ArrowDown: [0, 1],
    };
    if (
      !directions[event.key] ||
      !snapshot ||
      locked ||
      !valid ||
      mode !== "region"
    )
      return;
    event.preventDefault();
    const delta = directions[event.key].map(
      (value) => value * (event.shiftKey ? 10 : 1),
    ) as Point;
    onChange(movedRegion(region, delta, snapshot.width, snapshot.height));
  }

  const content = (
    <Box className={`snapshot-editor ${expanded ? "expanded" : ""}`}>
      <Stack
        className="editor-toolbar"
        direction="row"
        useFlexGap
        sx={{
          gap: 1,
          flexWrap: "wrap",
          alignItems: "center",
          justifyContent: "space-between",
        }}
      >
        <ToggleButtonGroup
          size="small"
          exclusive
          value={mode}
          onChange={(_, value) => {
            if (value) {
              setMode(value);
              drag.current = null;
              setDraft(null);
            }
          }}
          aria-label="截图操作工具"
        >
          <ToggleButton value="region" disabled={locked}>
            <CropFreeIcon fontSize="small" />
            框选
          </ToggleButton>
          <ToggleButton value="color" disabled={locked}>
            <ColorizeIcon fontSize="small" />
            取色
          </ToggleButton>
          <ToggleButton value="pan">
            <PanToolOutlinedIcon fontSize="small" />
            平移
          </ToggleButton>
        </ToggleButtonGroup>
        <Stack direction="row" sx={{ alignItems: "center", gap: 0.5 }}>
          <IconButton
            aria-label="缩小截图"
            size="small"
            disabled={!snapshot || (zoom !== null && zoom <= 0.125)}
            onClick={() => changeZoom((zoom ?? scale) / 1.5)}
          >
            <ZoomOutIcon />
          </IconButton>
          <Typography
            variant="caption"
            sx={{
              minWidth: 65,
              textAlign: "center",
              fontVariantNumeric: "tabular-nums",
            }}
          >
            {zoom === null ? "适应窗口" : `${Math.round(zoom * 100)}%`}
          </Typography>
          <IconButton
            aria-label="放大截图"
            size="small"
            disabled={!snapshot || (zoom !== null && zoom >= 16)}
            onClick={() => changeZoom((zoom ?? scale) * 1.5)}
          >
            <ZoomInIcon />
          </IconButton>
          <Button disabled={!snapshot} onClick={() => changeZoom(1)}>
            1:1
          </Button>
          <Button disabled={!snapshot} onClick={() => changeZoom(null)}>
            适应
          </Button>
          <Tooltip title={expanded ? "退出全屏（Esc）" : "全屏编辑"}>
            <IconButton
              aria-label={expanded ? "退出全屏" : "全屏编辑"}
              onClick={() => setExpanded(!expanded)}
            >
              {expanded ? <CloseIcon /> : <FullscreenIcon />}
            </IconButton>
          </Tooltip>
        </Stack>
      </Stack>
      {imageError && <Alert severity="error">{imageError}</Alert>}
      <Box
        className="preview"
        ref={viewport}
        sx={{ borderColor: "divider", bgcolor: "action.hover" }}
      >
        {snapshot ? (
          <div
            ref={stage}
            className={`image-stage tool-${mode}`}
            style={{
              width: zoom === null ? "100%" : snapshot.width * zoom,
              aspectRatio: `${snapshot.width} / ${snapshot.height}`,
            }}
            tabIndex={0}
            role="group"
            aria-label="截图编辑画布，拖动框选；选中区域内拖动移动；方向键微调，Shift 加速，Escape 取消"
            onPointerDown={pointerDown}
            onPointerMove={pointerMove}
            onPointerUp={(e) => finish(e)}
            onPointerCancel={(e) => finish(e, true)}
            onLostPointerCapture={() => {
              drag.current = null;
              setDraft(null);
            }}
            onPointerLeave={() => {
              if (!drag.current) setHover(null);
            }}
            onKeyDown={keyDown}
          >
            <img
              src={snapshot.data_url}
              alt="当前角色的原始截图"
              draggable={false}
            />
            {valid && (
              <>
                <div
                  className="selection-box"
                  data-move="true"
                  style={{
                    left: `${(current.start[0] / snapshot.width) * 100}%`,
                    top: `${(current.start[1] / snapshot.height) * 100}%`,
                    width: `${((current.end[0] - current.start[0] + 1) / snapshot.width) * 100}%`,
                    height: `${((current.end[1] - current.start[1] + 1) / snapshot.height) * 100}%`,
                    pointerEvents:
                      mode === "region" && !locked ? "auto" : "none",
                  }}
                />
                {mode === "region" &&
                  !locked &&
                  handles.map((handle) => {
                    const p = handlePosition(current, handle);
                    return (
                      <div
                        key={handle}
                        className={`selection-handle handle-${handle}`}
                        data-handle={handle}
                        style={{
                          left: `${(p[0] / snapshot.width) * 100}%`,
                          top: `${(p[1] / snapshot.height) * 100}%`,
                        }}
                      />
                    );
                  })}
              </>
            )}
          </div>
        ) : (
          <Stack
            sx={{
              minHeight: 300,
              alignItems: "center",
              justifyContent: "center",
              px: 3,
              textAlign: "center",
              gap: 1,
            }}
          >
            <ImageSearchOutlinedIcon
              sx={{ fontSize: 48, color: "text.disabled", mb: 1 }}
            />
            <Typography variant="h6">从一张截图开始</Typography>
            <Typography variant="body2" color="text.secondary">
              为角色选择窗口或屏幕，再点击「刷新截图」。
            </Typography>
            <Typography variant="caption" color="text.secondary">
              画面仅在本机用于区域配置与 RGB 取色
            </Typography>
          </Stack>
        )}
      </Box>
      <Stack
        direction="row"
        useFlexGap
        sx={{
          flexWrap: "wrap",
          gap: 1,
          alignItems: "center",
          justifyContent: "space-between",
          py: 1,
        }}
      >
        <Typography variant="caption" color="text.secondary">
          {locked
            ? "监控中仅可查看，停止后可编辑"
            : mode === "color"
              ? "点击像素添加 RGB；放大镜显示原始截图颜色"
              : mode === "pan"
                ? "拖动浏览截图；鼠标中键也可随时平移"
                : "拖动新建 · 框内移动 · 控制点缩放 · Shift 拖动重画 · 方向键微调 · Esc 取消"}
        </Typography>
        <Typography variant="caption" color="text.secondary">
          {snapshot
            ? `${snapshot.width} × ${snapshot.height} px`
            : "原始像素坐标"}
        </Typography>
      </Stack>
      {hover && imageReady && (
        <Stack
          direction="row"
          className="pixel-inspector"
          sx={{
            alignItems: "center",
            gap: 1.5,
            bgcolor: "action.hover",
            borderRadius: 2,
            p: 1,
            mb: 2,
          }}
        >
          <canvas
            ref={magnifier}
            width={99}
            height={99}
            aria-label="光标处像素放大镜"
          />
          <Box>
            <Typography variant="caption" color="text.secondary">
              像素放大镜 · 9×
            </Typography>
            <Typography variant="body2" sx={{ fontFamily: "monospace" }}>
              X {hover.point[0]} / Y {hover.point[1]}
            </Typography>
            <Stack direction="row" sx={{ alignItems: "center", gap: 1 }}>
              <span
                className="color-swatch"
                style={{ backgroundColor: colorHex(hover.rgb) }}
              />
              <Typography variant="caption">
                RGB {hover.rgb.join(", ")} · {colorHex(hover.rgb)}
              </Typography>
            </Stack>
          </Box>
        </Stack>
      )}
      <Divider sx={{ mb: 2 }} />
      <Stack
        direction="row"
        useFlexGap
        sx={{ gap: 1.5, alignItems: "center", flexWrap: "wrap", mb: 2 }}
      >
        {(["start", "end"] as const).map((key) =>
          ([0, 1] as const).map((axis) => (
            <TextField
              key={`${key}-${axis}`}
              label={`${key === "start" ? "起点" : "终点"} ${axis === 0 ? "X" : "Y"}`}
              type="number"
              disabled={locked}
              value={current[key][axis]}
              error={!valid}
              sx={{ width: 118 }}
              slotProps={{
                htmlInput: {
                  min: 0,
                  max: snapshot
                    ? (axis === 0 ? snapshot.width : snapshot.height) - 1
                    : undefined,
                  step: 1,
                },
              }}
              onChange={(e) => {
                const numeric = Number(e.target.value);
                if (!Number.isFinite(numeric)) return;
                const value = clamp(
                  Math.trunc(numeric),
                  0,
                  snapshot
                    ? (axis === 0 ? snapshot.width : snapshot.height) - 1
                    : Number.MAX_SAFE_INTEGER,
                );
                const next = {
                  ...region,
                  start: [...region.start] as Point,
                  end: [...region.end] as Point,
                };
                next[key][axis] = value;
                if (key === "start" && value > next.end[axis])
                  next.end[axis] = value;
                if (key === "end" && value < next.start[axis])
                  next.start[axis] = value;
                onChange(next);
              }}
            />
          )),
        )}
        <Chip
          size="small"
          variant="outlined"
          color={valid ? "default" : "error"}
          label={
            valid
              ? `${current.end[0] - current.start[0] + 1} × ${current.end[1] - current.start[1] + 1} px`
              : "区域超出截图"
          }
        />
      </Stack>
      {!valid && (
        <Alert severity="warning" sx={{ mb: 2 }}>
          区域不在当前截图内，请重新框选或调整坐标。后端会暂停越界角色的识别。
        </Alert>
      )}
      <Typography variant="subtitle2" sx={{ mb: 1 }}>
        目标颜色{" "}
        <Typography component="span" variant="caption" color="text.secondary">
          / RGB 精确匹配
        </Typography>
      </Typography>
      <Stack
        direction="row"
        useFlexGap
        sx={{ gap: 1, flexWrap: "wrap", alignItems: "center" }}
      >
        {region.rgb.map((rgb, index) => (
          <Box
            className="color-entry"
            key={index}
            sx={{ borderColor: "divider" }}
          >
            <input
              aria-label={`目标颜色 ${index + 1}`}
              type="color"
              disabled={locked}
              value={colorHex(rgb)}
              onChange={(e) =>
                onChange({
                  ...region,
                  rgb: region.rgb.map((value, i) =>
                    i === index ? hexRgb(e.target.value) : value,
                  ),
                })
              }
            />
            <Typography variant="caption" sx={{ fontFamily: "monospace" }}>
              {rgb.join(", ")}
            </Typography>
            <IconButton
              size="small"
              aria-label={`删除颜色 ${index + 1}`}
              disabled={locked || region.rgb.length <= 1}
              onClick={() =>
                onChange({
                  ...region,
                  rgb: region.rgb.filter((_, i) => i !== index),
                })
              }
            >
              <CloseIcon sx={{ fontSize: 16 }} />
            </IconButton>
          </Box>
        ))}
        <Button
          startIcon={<AddIcon />}
          disabled={locked}
          onClick={() =>
            onChange({ ...region, rgb: [...region.rgb, [255, 255, 255]] })
          }
        >
          添加颜色
        </Button>
      </Stack>
      {!region.rgb.length && (
        <Alert severity="warning" sx={{ mt: 1 }}>
          请添加至少一种目标颜色。
        </Alert>
      )}
      <Typography
        variant="caption"
        color="text.secondary"
        sx={{ display: "block", mt: 1.5 }}
      >
        区域包含起点与终点。任一像素匹配列表中的任一种 RGB 即命中，忽略 alpha。
      </Typography>
    </Box>
  );
  return expanded ? (
    <Dialog
      open
      fullScreen
      onClose={() => setExpanded(false)}
      aria-label="截图区域全屏编辑"
    >
      {content}
    </Dialog>
  ) : (
    content
  );
}
