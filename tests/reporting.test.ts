import assert from "node:assert/strict";
import { test } from "node:test";
import { copyCharacterRegions, newCharacter } from "../src/reporting.ts";

test("覆盖已有角色仅替换识别区域，保留目标身份与禁用状态，且不共享嵌套数据", () => {
  const source = newCharacter(1);
  source.warn_region = { start: [2, 3], end: [8, 9], rgb: [[10, 20, 30]] };
  source.reminder_now_region = {
    start: [4, 5],
    end: [6, 7],
    rgb: [[40, 50, 60]],
  };
  source.reminder_enemy_region = {
    start: [9, 10],
    end: [11, 12],
    rgb: [[70, 80, 90]],
  };
  source.reminder_gate_region = {
    start: [13, 14],
    end: [15, 16],
    rgb: [[100, 110, 120]],
  };
  const target = { ...newCharacter(2), enabled: false };
  const sourceBefore = structuredClone(source);
  const targetBefore = structuredClone(target);

  const copied = copyCharacterRegions(source, target);
  assert.equal(copied.title, target.title);
  assert.equal(copied.enabled, false);
  for (const key of [
    "warn_region",
    "reminder_now_region",
    "reminder_gate_region",
    "reminder_enemy_region",
  ] as const) {
    assert.deepEqual(copied[key], source[key]);
    copied[key].start[0] += 1;
    copied[key].end[1] += 1;
    copied[key].rgb[0][0] += 1;
    copied[key].rgb.push([100, 110, 120]);
  }
  assert.deepEqual(source, sourceBefore);
  assert.deepEqual(target, targetBefore);
});
