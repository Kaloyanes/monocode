import { Redirect, useLocalSearchParams } from "expo-router";
import { useHeaderHeight } from "expo-router/react-navigation";
import { View } from "react-native";

import { DraftScreen } from "@/features/drafts/DraftScreen";

export default function DraftRoute() {
  const { conversationId, draft } = useLocalSearchParams<{
    conversationId: string;
    draft: string;
    title?: string;
  }>();
  // The sheet's header is transparent: the page starts below it.
  const headerHeight = useHeaderHeight();
  if (!conversationId || !draft) return <Redirect href="/" />;
  return (
    <View style={{ flex: 1, paddingTop: headerHeight }}>
      <DraftScreen conversationId={conversationId} draft={draft} />
    </View>
  );
}
