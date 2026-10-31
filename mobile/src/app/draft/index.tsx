import { Redirect, useLocalSearchParams } from "expo-router";

import { DraftScreen } from "@/features/drafts/DraftScreen";

export default function DraftRoute() {
  const { conversationId, draft } = useLocalSearchParams<{
    conversationId: string;
    draft: string;
    title?: string;
  }>();
  if (!conversationId || !draft) return <Redirect href="/" />;
  return <DraftScreen conversationId={conversationId} draft={draft} />;
}
