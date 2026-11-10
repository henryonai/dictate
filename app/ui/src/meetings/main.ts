import { mount } from "svelte";
import "../app.css";
import MeetingsApp from "./MeetingsApp.svelte";

const target = document.getElementById("app");
if (!target) {
  throw new Error("The page has no #app element.");
}

export default mount(MeetingsApp, { target });
