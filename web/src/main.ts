import "./app.css";
import { mount } from "svelte";
import App from "./App.svelte";

// Svelte 5 mounts explicitly rather than via `new App(...)`.
export default mount(App, {
  target: document.getElementById("app")!,
});
