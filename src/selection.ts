import type { Region } from "./reporting";

export type Point = [number, number];
export type Handle = "nw" | "n" | "ne" | "e" | "se" | "s" | "sw" | "w";
export const handles: Handle[] = ["nw", "n", "ne", "e", "se", "s", "sw", "w"];

export function clamp(value: number, min: number, max: number) {
  return Math.min(max, Math.max(min, value));
}

export function pixelPoint(
  x: number,
  y: number,
  bounds: { left: number; top: number; width: number; height: number },
  width: number,
  height: number,
): Point {
  return [
    clamp(Math.floor(((x - bounds.left) * width) / bounds.width), 0, width - 1),
    clamp(
      Math.floor(((y - bounds.top) * height) / bounds.height),
      0,
      height - 1,
    ),
  ];
}

export function drawnRegion(region: Region, from: Point, to: Point): Region {
  return {
    ...region,
    start: [Math.min(from[0], to[0]), Math.min(from[1], to[1])],
    end: [Math.max(from[0], to[0]), Math.max(from[1], to[1])],
  };
}

export function validRegion(region: Region, width: number, height: number) {
  return (
    region.start.every(
      (value, axis) =>
        Number.isInteger(value) && value >= 0 && value <= region.end[axis],
    ) &&
    region.end.every(
      (value, axis) =>
        Number.isInteger(value) && value < (axis === 0 ? width : height),
    )
  );
}

export function movedRegion(
  region: Region,
  delta: Point,
  width: number,
  height: number,
): Region {
  const dx = clamp(delta[0], -region.start[0], width - 1 - region.end[0]);
  const dy = clamp(delta[1], -region.start[1], height - 1 - region.end[1]);
  return {
    ...region,
    start: [region.start[0] + dx, region.start[1] + dy],
    end: [region.end[0] + dx, region.end[1] + dy],
  };
}

export function resizedRegion(
  region: Region,
  handle: Handle,
  delta: Point,
  width: number,
  height: number,
): Region {
  const from: Point = [...region.start];
  const to: Point = [...region.end];
  // 使用拖动增量，避免右/下控制点处于像素外边界时点击即偏移一像素。
  if (handle.includes("w")) from[0] = clamp(from[0] + delta[0], 0, to[0]);
  if (handle.includes("e")) to[0] = clamp(to[0] + delta[0], from[0], width - 1);
  if (handle.includes("n")) from[1] = clamp(from[1] + delta[1], 0, to[1]);
  if (handle.includes("s"))
    to[1] = clamp(to[1] + delta[1], from[1], height - 1);
  return { ...region, start: from, end: to };
}

// overlay 的右/下边界比包含的最后一个像素多 1。
export function handlePosition(region: Region, handle: Handle): Point {
  return [
    handle.includes("w")
      ? region.start[0]
      : handle.includes("e")
        ? region.end[0] + 1
        : (region.start[0] + region.end[0] + 1) / 2,
    handle.includes("n")
      ? region.start[1]
      : handle.includes("s")
        ? region.end[1] + 1
        : (region.start[1] + region.end[1] + 1) / 2,
  ];
}
