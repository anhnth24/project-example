import React from "react";
import ReactDOM from "react-dom/client";
// Fonts bundle offline (theo skill ui-ux-pro-max: Inter cho UI/body, Plus Jakarta Sans cho heading).
import "@fontsource-variable/inter";
import "@fontsource-variable/plus-jakarta-sans";
import App from "./App";
import "./styles.css";

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>
);
