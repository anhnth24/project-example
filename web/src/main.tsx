import { createRoot } from "react-dom/client";
import "./styles.css";

function App() {
  return (
    <main>
      <p>Markhand Web</p>
      <h1>Web shell is ready for the Phase 2 SPA.</h1>
    </main>
  );
}

createRoot(document.getElementById("root")!).render(<App />);
