// The font ships inside the app rather than being asked of the system: a
// packaged build (an AppImage with its own Pango, a Mac with no JetBrains Mono
// installed) otherwise renders in whatever monospace happens to be around.
// Served from the app's own origin, so nothing is fetched and CSP is unchanged.
import "@fontsource-variable/jetbrains-mono/wght.css";
import "@fontsource-variable/jetbrains-mono/wght-italic.css";
import "./app.css";
import { mount } from "svelte";
import App from "./App.svelte";

// Svelte 5 mounts explicitly rather than via `new App(...)`.
export default mount(App, {
  target: document.getElementById("app")!,
});
