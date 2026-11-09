import {
  addClipboardListener,
  ContentType,
  getImageAsync,
  hasImageAsync,
} from "expo-clipboard";
import { File, Paths } from "expo-file-system";
import { ImageManipulator, SaveFormat } from "expo-image-manipulator";
import {
  launchImageLibraryAsync,
  VideoExportPreset,
  type ImagePickerAsset,
} from "expo-image-picker";
import { AppState } from "react-native";

/** A file or photo held in the composer until it's sent. */
export type Attachment = {
  id: string;
  name: string;
  mimeType: string;
  size: number;
  /** On this phone, for the composer's thumbnail. */
  uri: string;
  /** The file's bytes, as the desktop takes them. */
  data: string;
};

/** What the desktop is sent for each: it stages them as its own composer does. */
export type WireAttachment = Pick<Attachment, "name" | "mimeType" | "data">;

export const MAX_ATTACHMENTS = 10;
/** Per file, before it travels as base64. */
export const MAX_FILE_BYTES = 10 * 1024 * 1024;
/** The whole message's, under the link's request limit with room for the text. */
const MAX_TOTAL_BASE64 = 24 * 1024 * 1024;
/** A video can take the whole message: the most bytes that fit as base64. */
const MAX_VIDEO_BYTES = (MAX_TOTAL_BASE64 / 4) * 3;
/** A photo's longer side; larger ones are scaled down, as vision models do anyway. */
const MAX_IMAGE_SIDE = 2048;
/** Sent as picked when already small and in a format every provider reads. */
const SENT_AS_IS = new Set(["image/png", "image/jpeg", "image/gif", "image/webp"]);
const MAX_AS_IS_BYTES = 1.5 * 1024 * 1024;

const MIME_BY_EXT: Record<string, string> = {
  png: "image/png",
  jpg: "image/jpeg",
  jpeg: "image/jpeg",
  gif: "image/gif",
  webp: "image/webp",
  heic: "image/heic",
  mp4: "video/mp4",
  mov: "video/quicktime",
  pdf: "application/pdf",
  txt: "text/plain",
  md: "text/markdown",
  csv: "text/csv",
  json: "application/json",
  html: "text/html",
  zip: "application/zip",
};

export const isImage = (attachment: Pick<Attachment, "mimeType">) =>
  attachment.mimeType.startsWith("image/");

export const toWire = (attachments: Attachment[]): WireAttachment[] =>
  attachments.map(({ name, mimeType, data }) => ({ name, mimeType, data }));

/**
 * Photos and videos from the library. Photos are scaled down and re-encoded
 * where they need it. iOS exports videos at 540p, which keeps a screen
 * recording readable and fits about half a minute of busy footage in a
 * message; Android hands over the original.
 */
export async function pickMedia(room: number): Promise<Attachment[]> {
  const result = await launchImageLibraryAsync({
    mediaTypes: ["images", "videos"],
    allowsMultipleSelection: true,
    selectionLimit: room,
    quality: 1,
    videoExportPreset: VideoExportPreset.H264_960x540,
  });
  if (result.canceled) return [];
  return Promise.all(
    result.assets
      .slice(0, room)
      .map((asset) => (asset.type === "video" ? fromVideo(asset) : fromPhoto(asset))),
  );
}

/** Any files, from Files and its providers. */
export async function pickFiles(room: number): Promise<Attachment[]> {
  const picked = await File.pickFileAsync({ multipleFiles: true });
  if (picked.canceled) return [];
  const files = picked.result.slice(0, room);
  const tooLarge = files.find((file) => file.size > MAX_FILE_BYTES);
  if (tooLarge) throw new Error(`${tooLarge.name} is over the 10 MB limit.`);
  return Promise.all(
    files.map(async (file) => ({
      id: makeId(),
      name: file.name,
      mimeType: mimeFor(file.name, file.type),
      size: file.size,
      uri: file.uri,
      data: await file.base64(),
    })),
  );
}

/**
 * The clipboard's image (a screenshot, a photo copied from Photos), as if it
 * had been picked: the same scaling, re-encoding and limits.
 * iOS asks the user to allow the paste the first time; a refusal reads as no image.
 */
export async function pasteImage(room: number): Promise<Attachment[]> {
  if (room <= 0) return [];
  const image = await getImageAsync({ format: "png" });
  if (!image) throw new Error("There's no image on the clipboard to paste.");
  const file = new File(Paths.cache, `pasted-${makeId()}.png`);
  // Arrives as a data URI; the file takes the bytes after its prefix.
  file.write(image.data.slice(image.data.indexOf(",") + 1), { encoding: "base64" });
  const discard = () => {
    try {
      if (file.exists) file.delete();
    } catch {
      // Only a cache file; the OS clears the cache in time anyway.
    }
  };
  try {
    const attachment = await fromPhoto({
      uri: file.uri,
      width: image.size.width,
      height: image.size.height,
      mimeType: "image/png",
      fileName: "Pasted image.png",
      fileSize: file.size,
    });
    // Re-encoded to its own file: the pasted PNG is no longer the thumbnail.
    if (attachment.uri !== file.uri) discard();
    return [attachment];
  } catch (reason) {
    discard();
    throw reason;
  }
}

/**
 * Tells whether the clipboard holds an image: now, whenever it changes, and
 * on coming back to the app (copied elsewhere). Only asks, never reads, so iOS
 * doesn't prompt to allow a paste until one is chosen.
 */
export function watchClipboardImage(onChange: (hasImage: boolean) => void): () => void {
  let live = true;
  const check = () =>
    void hasImageAsync().then(
      (has) => live && onChange(has),
      () => live && onChange(false),
    );
  check();
  const clipboard = addClipboardListener(({ contentTypes }) => {
    if (live) onChange(contentTypes.includes(ContentType.IMAGE));
  });
  const app = AppState.addEventListener("change", (state) => {
    if (state === "active") check();
  });
  return () => {
    live = false;
    clipboard.remove();
    app.remove();
  };
}

/** Adds picks to what's held, refusing any that would take the message over the link's limit. */
export function merge(existing: Attachment[], added: Attachment[]): Attachment[] {
  const next = [...existing, ...added].slice(0, MAX_ATTACHMENTS);
  const total = next.reduce((sum, attachment) => sum + attachment.data.length, 0);
  if (total > MAX_TOTAL_BASE64)
    throw new Error("These attachments are too large to send together. Remove some and try again.");
  return next;
}

async function fromVideo(asset: ImagePickerAsset): Promise<Attachment> {
  const file = new File(asset.uri);
  const name = asset.fileName ?? file.name;
  // Checked before reading: an Android original can be gigabytes.
  if (file.size > MAX_VIDEO_BYTES) {
    const limit = MAX_VIDEO_BYTES / (1024 * 1024);
    throw new Error(
      `${name} is too long to send. A video can be up to ${limit} MB, so trim it and try again.`,
    );
  }
  return {
    id: makeId(),
    name,
    mimeType: asset.mimeType ?? mimeFor(name, null),
    size: file.size,
    uri: asset.uri,
    data: await file.base64(),
  };
}

type PhotoSource = Pick<ImagePickerAsset, "uri" | "width" | "height"> &
  Partial<Pick<ImagePickerAsset, "mimeType" | "fileName" | "fileSize">>;

async function fromPhoto(asset: PhotoSource): Promise<Attachment> {
  const mimeType = asset.mimeType ?? mimeFor(asset.fileName ?? "", null);
  const small =
    SENT_AS_IS.has(mimeType) &&
    (asset.fileSize ?? Infinity) <= MAX_AS_IS_BYTES &&
    Math.max(asset.width, asset.height) <= MAX_IMAGE_SIDE;
  if (small) {
    const file = new File(asset.uri);
    return {
      id: makeId(),
      name: asset.fileName ?? `photo.${mimeType.split("/")[1]}`,
      mimeType,
      size: file.size,
      uri: asset.uri,
      data: await file.base64(),
    };
  }
  const context = ImageManipulator.manipulate(asset.uri);
  if (Math.max(asset.width, asset.height) > MAX_IMAGE_SIDE)
    context.resize(
      asset.width >= asset.height ? { width: MAX_IMAGE_SIDE } : { height: MAX_IMAGE_SIDE },
    );
  const saved = await (await context.renderAsync()).saveAsync({
    format: SaveFormat.JPEG,
    compress: 0.85,
    base64: true,
  });
  const base = (asset.fileName ?? "photo").replace(/\.[^.]+$/, "");
  return {
    id: makeId(),
    name: `${base}.jpg`,
    mimeType: "image/jpeg",
    size: Math.floor(((saved.base64 ?? "").length * 3) / 4),
    uri: saved.uri,
    data: saved.base64 ?? "",
  };
}

function mimeFor(name: string, type: string | null | undefined): string {
  if (type && type !== "application/octet-stream") return type;
  const ext = name.split(".").pop()?.toLowerCase() ?? "";
  return MIME_BY_EXT[ext] ?? "application/octet-stream";
}

const makeId = () => `${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 8)}`;
