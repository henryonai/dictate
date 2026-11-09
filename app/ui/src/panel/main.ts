import { mount } from "svelte";
import "./panel.css";
import PanelApp from "./PanelApp.svelte";

const target = document.getElementById("app");
if (!target) {
  throw new Error("The page has no #app element.");
}

// The Rust side opens this page once for each floating window: `panel.html?view=bubble` and `panel.html?view=card`.
const view = new URLSearchParams(window.location.search).get("view");

export default mount(PanelApp, { target, props: { view } });
