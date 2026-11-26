import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import { I18nProvider, startingLanguage } from "./lib/i18n/I18n";
import "./styles.css";

// The language first (a local IPC call), so the first screen is already in it.
startingLanguage().then((language) => {
  ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
    <React.StrictMode>
      <I18nProvider initial={language}>
        <App />
      </I18nProvider>
    </React.StrictMode>,
  );
});
