import { router } from "expo-router";

/** Opens a draft card from a conversation as a modal over it. */
export function openDraft(conversationId: string, draft: string, title?: string) {
  router.push({ pathname: "/draft", params: { conversationId, draft, title: title ?? "" } });
}
