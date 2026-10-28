import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { BrowserRouter } from "react-router";
import { SessionProvider } from "./session";
import { App } from "./shell";
import "./style.css";

const root = document.querySelector("#root");
if (root) {
  createRoot(root).render(
    <StrictMode>
      <BrowserRouter>
        <SessionProvider>
          <App />
        </SessionProvider>
      </BrowserRouter>
    </StrictMode>,
  );
}
