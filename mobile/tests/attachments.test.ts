import { beforeEach, expect, test, vi } from "vitest";

const runtime = vi.hoisted(() => ({
  launch: vi.fn(),
  files: {} as Record<string, number>,
  read: vi.fn(async () => "BYTES"),
}));

vi.mock("expo-image-picker", () => ({
  launchImageLibraryAsync: runtime.launch,
  VideoExportPreset: { H264_960x540: 5 },
}));
vi.mock("expo-image-manipulator", () => ({ ImageManipulator: {}, SaveFormat: {} }));
vi.mock("expo-file-system", () => ({
  File: class {
    uri: string;
    constructor(uri: string) {
      this.uri = uri;
    }
    get name() {
      return this.uri.split("/").pop();
    }
    get size() {
      return runtime.files[this.uri];
    }
    base64() {
      return runtime.read();
    }
  },
}));

import { merge, pickMedia } from "@/features/sessions/attachments";

const MB = 1024 * 1024;
const video = (uri: string, size: number, more: object = {}) => {
  runtime.files[uri] = size;
  return { type: "video", uri, width: 540, height: 960, ...more };
};

beforeEach(() => {
  runtime.launch.mockReset();
  runtime.read.mockClear();
  runtime.files = {};
});

test("the library offers videos as well as photos, compressed on export", async () => {
  runtime.launch.mockResolvedValue({ canceled: true, assets: null });
  await pickMedia(3);
  expect(runtime.launch).toHaveBeenCalledWith(
    expect.objectContaining({
      mediaTypes: ["images", "videos"],
      selectionLimit: 3,
      videoExportPreset: 5,
    }),
  );
});

test("a video travels as its own bytes, under the name and type it was exported with", async () => {
  runtime.launch.mockResolvedValue({
    canceled: false,
    assets: [video("file:///a.mp4", 5 * MB, { fileName: "IMG_1.mp4", mimeType: "video/mp4" })],
  });
  const [attachment] = await pickMedia(10);
  expect(attachment).toMatchObject({
    name: "IMG_1.mp4",
    mimeType: "video/mp4",
    size: 5 * MB,
    uri: "file:///a.mp4",
    data: "BYTES",
  });
});

test("a video without a name or type takes them from its file", async () => {
  runtime.launch.mockResolvedValue({ canceled: false, assets: [video("file:///b.mov", MB)] });
  const [attachment] = await pickMedia(10);
  expect(attachment).toMatchObject({ name: "b.mov", mimeType: "video/quicktime" });
});

test("a video that fills the message fits the limit merge enforces", async () => {
  runtime.read.mockResolvedValueOnce("x".repeat(24 * MB));
  runtime.launch.mockResolvedValue({ canceled: false, assets: [video("file:///c.mp4", 18 * MB)] });
  const picked = await pickMedia(10);
  expect(merge([], picked)).toHaveLength(1);
});

test("a video over the limit is refused by name, before its bytes are read", async () => {
  runtime.launch.mockResolvedValue({
    canceled: false,
    assets: [video("file:///d.mp4", 18 * MB + 1, { fileName: "long.mp4" })],
  });
  await expect(pickMedia(10)).rejects.toThrow("long.mp4 is too long to send. A video can be up to 18 MB");
  expect(runtime.read).not.toHaveBeenCalled();
});
