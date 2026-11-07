import { countOnlyRegisteredViews } from "./reanimatedSettledProps";

// A require, as an import would pull Reanimated's raw source into the type
// check, which doesn't compile under this project's settings.
countOnlyRegisteredViews(
  // eslint-disable-next-line @typescript-eslint/no-require-imports
  require("react-native-reanimated/src/PropsRegistryGarbageCollector")
    .PropsRegistryGarbageCollector,
);
