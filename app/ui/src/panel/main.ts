import { mount } from "svelte";
import "../tokens.css";
import "./panel.css";
import { followTheme } from "../lib/theme";
import PanelApp from "./PanelApp.svelte";

const target = document.getElementById("app");
if (!target) {
  throw new Error("The page has no #app element.");
}

// The theme that the settings ask for, now and when it changes (the panels are windows of their own, with no copy of the settings).
void followTheme();

// The Rust side opens this page once for each floating window: `panel.html?view=bubble` and `panel.html?view=card`.
const view = new URLSearchParams(window.location.search).get("view");

export default mount(PanelApp, { target, props: { view } });
