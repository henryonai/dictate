import { mount } from "svelte";
import "../tokens.css";
import "./recorder.css";
import { followTheme } from "../lib/theme";
import RecorderApp from "./RecorderApp.svelte";

const target = document.getElementById("app");
if (!target) {
  throw new Error("The page has no #app element.");
}

// The theme that the settings ask for, now and when it changes (the bar is a window of its own, with no copy of the settings).
void followTheme();

export default mount(RecorderApp, { target });
