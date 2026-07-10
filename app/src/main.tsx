import React from "react";
import ReactDOM from "react-dom/client";
// Astryx design system (thứ tự layer: reset -> base -> theme tokens).
import "@astryxdesign/core/reset.css";
import "@astryxdesign/core/astryx.css";
import "@astryxdesign/theme-neutral/theme.css";
import { Theme } from "@astryxdesign/core/theme";
import { neutralTheme } from "@astryxdesign/theme-neutral/built";
// Fonts bundle offline.
import "@fontsource-variable/inter";
import "@fontsource-variable/plus-jakarta-sans";
import App from "./App";
import "./styles.css";

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    {/* mode=light cố định: app chỉ có giao diện sáng; mặc định "system" sẽ
        ăn theo dark mode của Windows làm token chữ sáng đè lên nền trắng */}
    <Theme theme={neutralTheme} mode="light">
      <App />
    </Theme>
  </React.StrictMode>
);
