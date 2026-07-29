import { AppShell } from "./components/shell/AppShell";
import { NavigationProvider } from "./components/providers/NavigationProvider";

function App() {
  return (
    <NavigationProvider>
      <AppShell />
    </NavigationProvider>
  );
}

export default App;
