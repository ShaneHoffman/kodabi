import { AppShell } from "./components/AppShell";
import { NavigationProvider } from "./components/NavigationProvider";

function App() {
  return (
    <NavigationProvider>
      <AppShell />
    </NavigationProvider>
  );
}

export default App;
