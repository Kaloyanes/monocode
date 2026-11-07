// The background push task must be defined before anything else runs, as a
// push can launch the app straight into it.
import "./src/features/liveActivity/backgroundTask";
import "./src/ui/applyReanimatedSettledProps";
import "expo-router/entry";
