import "@fontsource-variable/atkinson-hyperlegible-next";
import "@fontsource-variable/big-shoulders";
import "./styles.css";
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { App } from "./App";
import { loadConfig } from "./config";

const root = createRoot(document.getElementById("root") as HTMLElement);

loadConfig()
  .then((config) =>
    root.render(
      <StrictMode>
        <App config={config} />
      </StrictMode>,
    ),
  )
  .catch((err: unknown) =>
    root.render(
      <main className="fatal">
        <h1>The blog isn't configured</h1>
        <p>{err instanceof Error ? err.message : String(err)}</p>
      </main>,
    ),
  );
