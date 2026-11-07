import type { NativeStackNavigationProp } from "expo-router/native-stack";
import type { ParamListBase } from "expo-router/react-navigation";
import { router, Stack, useLocalSearchParams, useNavigation } from "expo-router";
import { useEffect } from "react";
import { useSafeAreaInsets } from "react-native-safe-area-context";

import { InboxPage, SCOPE_SPACE } from "@/features/inbox/InboxPage";
import { useInboxSupported } from "@/features/inbox/InboxProvider";
import { SheetLayout } from "@/features/sessions/SessionScreen";
import { useDrawerActions } from "@/ui/ScenePager";
import { useNativeHeaderHeight } from "@/ui/useNativeHeaderHeight";

// The Inbox, pushed from its row in the drawer. `projectId` is the project in
// front when it was opened, which heads the page's project picker.
export default function InboxListScreen() {
  const { projectId } = useLocalSearchParams<{ projectId?: string }>();
  const supported = useInboxSupported();
  const insets = useSafeAreaInsets();
  const headerHeight = useNativeHeaderHeight();

  // The drawer held for this page shuts as its slide starts (see closeBehind).
  const navigation = useNavigation<NativeStackNavigationProp<ParamListBase>>();
  const { releaseDrawer } = useDrawerActions();
  useEffect(
    () =>
      navigation.addListener("transitionStart", (event) => {
        if (!event.data.closing) releaseDrawer();
      }),
    [navigation, releaseDrawer],
  );

  // The desktop can drop its Inbox while this is open.
  useEffect(() => {
    if (!supported && router.canGoBack()) router.back();
  }, [supported]);

  return (
    <>
      <Stack.Screen options={{ title: "Inbox" }} />
      <SheetLayout
        top={headerHeight + SCOPE_SPACE}
        // The search rests just above the keyboard, as the drawer's does.
        keyboardOffset={Math.max(insets.bottom, 12) - 10}
      >
        <InboxPage pinned={projectId} />
      </SheetLayout>
    </>
  );
}
