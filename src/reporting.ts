export type Region = {
  start: [number, number];
  end: [number, number];
  rgb: [number, number, number][];
};
export type Character = {
  title: string;
  warn_region: Region;
  reminder_now_region: Region;
  reminder_enemy_region: Region;
};
export type Method =
  | {
      type: "Onebot";
      url: string;
      token: string;
      target: number;
    }
  | { type: "Notification" }
  | { type: "Ntfy"; server_url: string; topic: string; token: string }
  | { type: "Voice"; warn_voice_path: string; reminder_voice_path: string }
  | { type: "Sse"; host: [number, number, number, number]; port: number }
  | {
      type: "ReverseWebsocket";
      url: string;
      try_forever: boolean;
      try_spacing: { secs: number; nanos: number };
    };
export type Config = { characters: Character[]; report_methods: Method[] };
export type ReportEvent = {
  type: "Warn" | "WarnCleared" | "Reminder";
  title: string;
};
export type Snapshot = { data_url: string; width: number; height: number };
export type Status = { running: boolean; sources: string[] };

declare global {
  interface Window {
    __TAURI__?: {
      core: {
        invoke: <T>(
          command: string,
          args?: Record<string, unknown>,
        ) => Promise<T>;
      };
      event: {
        listen: <T>(
          name: string,
          handler: (event: { payload: T }) => void,
        ) => Promise<() => void>;
      };
    };
  }
}
export function invoke<T>(
  command: string,
  args?: Record<string, unknown>,
): Promise<T> {
  if (!window.__TAURI__)
    return Promise.reject(new Error("请在 Tauri 桌面应用中使用此功能"));
  return window.__TAURI__.core.invoke<T>(command, args);
}
export const regionNames = {
  warn_region: "告警区域",
  reminder_now_region: "提醒准备区域",
  reminder_enemy_region: "敌人出现区域",
} as const;
export type RegionKey = keyof typeof regionNames;
export function newCharacter(index: number): Character {
  const region = (): Region => ({
    start: [0, 0],
    end: [0, 0],
    rgb: [[255, 0, 0]],
  });
  return {
    title: `EVE - 角色${index}`,
    warn_region: region(),
    reminder_now_region: region(),
    reminder_enemy_region: region(),
  };
}
export function colorHex(rgb: number[]): string {
  return `#${rgb.map((v) => v.toString(16).padStart(2, "0")).join("")}`;
}
export function hexRgb(hex: string): [number, number, number] {
  return [
    parseInt(hex.slice(1, 3), 16),
    parseInt(hex.slice(3, 5), 16),
    parseInt(hex.slice(5, 7), 16),
  ];
}
