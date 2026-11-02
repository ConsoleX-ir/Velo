import React from "react";
import ReactDOM from "react-dom/client";
import App from "../App";
import "../styles.css";

/*
 * Preview entry for preview.html only. The shim that precedes this module
 * (inline in preview.html) provides a toy Tauri IPC, so the real App runs
 * unmodified against simulated engine events. Production builds ship from
 * index.html and never include this file.
 */

// Simulate preference overrides when requested via query params (?theme=light, ?motion=reduced).
const params = new URLSearchParams(window.location.search);
if (params.get("motion") === "reduced") localStorage.setItem("velo-motion", "reduced");
if (params.get("theme") === "light") localStorage.setItem("velo-theme", "light");

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
