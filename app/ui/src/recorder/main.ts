import { mount } from "svelte";
import "./recorder.css";
import RecorderApp from "./RecorderApp.svelte";

const target = document.getElementById("app");
if (!target) {
  throw new Error("The page has no #app element.");
}

export default mount(RecorderApp, { target });
