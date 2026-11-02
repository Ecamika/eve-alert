import assert from "node:assert/strict";
import { test } from "node:test";
import {
  drawnRegion,
  handlePosition,
  movedRegion,
  pixelPoint,
  resizedRegion,
  validRegion,
} from "../src/selection.ts";
import type { Region } from "../src/reporting.ts";

const region: Region = { start: [10, 20], end: [19, 29], rgb: [[255, 0, 0]] };

test("缩放截图映射到原始像素，边缘与画布外坐标被限制在最后一个像素", () => {
  const bounds = { left: 100, top: 50, width: 200, height: 100 };
  assert.deepEqual(pixelPoint(200, 100, bounds, 1000, 500), [500, 250]);
  assert.deepEqual(pixelPoint(300, 150, bounds, 1000, 500), [999, 499]);
  assert.deepEqual(pixelPoint(-20, -20, bounds, 1000, 500), [0, 0]);
});

test("反向拖动规范化坐标，单像素选择仍包含两端", () => {
  assert.deepEqual(drawnRegion(region, [42, 31], [8, 9]), {
    ...region,
    start: [8, 9],
    end: [42, 31],
  });
  const single = drawnRegion(region, [99, 49], [99, 49]);
  assert.ok(validRegion(single, 100, 50));
  assert.deepEqual(handlePosition(single, "se"), [100, 50]);
});

test("移动在截图边缘停止，并保持原来的像素宽高与颜色", () => {
  assert.deepEqual(movedRegion(region, [-100, 100], 100, 50), {
    ...region,
    start: [0, 40],
    end: [9, 49],
  });
  assert.deepEqual(region.start, [10, 20]);
});

test("调整边角不会反转坐标或缩小到零像素", () => {
  assert.deepEqual(resizedRegion(region, "nw", [30, 40], 100, 50), {
    ...region,
    start: [19, 29],
  });
  assert.deepEqual(resizedRegion(region, "se", [-100, -100], 100, 50), {
    ...region,
    end: [10, 20],
  });
  assert.deepEqual(resizedRegion(region, "e", [31, 0], 100, 50), {
    ...region,
    end: [50, 29],
  });
  assert.deepEqual(resizedRegion(region, "se", [0, 0], 100, 50), region);
  assert.deepEqual(resizedRegion(region, "se", [100, 100], 100, 50), {
    ...region,
    end: [99, 49],
  });
});

test("越界、负数、非整数和反转区域不能视为有效区域", () => {
  assert.ok(validRegion(region, 20, 30));
  assert.equal(validRegion(region, 19, 30), false);
  for (const invalid of [
    { ...region, start: [-1, 0] },
    { ...region, start: [20, 20] },
    { ...region, end: [19.5, 29] },
    { ...region, end: [NaN, 29] },
  ])
    assert.equal(validRegion(invalid as Region, 100, 100), false);
});
