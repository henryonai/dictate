import { render } from "@testing-library/svelte";
import { expect, test } from "vitest";
import { barHeights } from "./state";
import Waveform from "./Waveform.svelte";

function heights(container: HTMLElement): number[] {
  return [...container.querySelectorAll<HTMLElement>(".bar")].map((bar) => parseFloat(bar.style.height));
}

test("waveform_bar_heights: seven bars, each 4 pixels high when nothing is heard", () => {
  const { container } = render(Waveform, { level: 0 });
  expect(heights(container)).toEqual([4, 4, 4, 4, 4, 4, 4]);
});

test("waveform_bar_heights: the bars follow the level by the formula", () => {
  const { container } = render(Waveform, { level: 1 });
  const wanted = barHeights(1);
  heights(container).forEach((height, i) => expect(height).toBeCloseTo(wanted[i], 1));
  // The tallest bar is 4 + 18.
  expect(Math.max(...heights(container))).toBeCloseTo(22, 1);
});

test("waveform_bar_heights: the bars are 3 wide and decoration, not something a screen reader reads", () => {
  const { container } = render(Waveform, { level: 0.3 });
  expect(container.firstElementChild).toHaveAttribute("aria-hidden", "true");
  for (const bar of container.querySelectorAll<HTMLElement>(".bar")) expect(bar.style.width).toBe("3px");
});
