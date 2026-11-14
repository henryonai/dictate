import { mount } from "svelte";
import "./app.css";
import App from "./App.svelte";
import { followTheme } from "./lib/theme";

const target = document.getElementById("app");
if (!target) {
  throw new Error("The page has no #app element.");
}

// The theme that the settings ask for, now and when it changes (a window follows it from the moment it opens).
void followTheme();

export default mount(App, { target });
