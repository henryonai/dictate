import { describe, expect, test } from "vitest";
import { acceleratorFromKeydown } from "./accelerator";
import type { KeyPress } from "./accelerator";

const press = (code: string, held: Partial<KeyPress> = {}): KeyPress => ({
  code,
  ctrlKey: false,
  altKey: false,
  shiftKey: false,
  metaKey: false,
  ...held,
});

describe("the accelerator of a key press", () => {
  test("Ctrl+Alt+Space", () => {
    expect(acceleratorFromKeydown(press("Space", { ctrlKey: true, altKey: true }))).toBe("Ctrl+Alt+Space");
  });

  test("the Command key is Super, as the shortcut plugin spells it", () => {
    expect(acceleratorFromKeydown(press("KeyD", { metaKey: true, shiftKey: true }))).toBe("Super+Shift+D");
  });

  test("modifiers come in one order, whichever way they were pressed", () => {
    const all = press("KeyK", { metaKey: true, ctrlKey: true, altKey: true, shiftKey: true });
    expect(acceleratorFromKeydown(all)).toBe("Super+Ctrl+Alt+Shift+K");
  });

  test("a function key needs no modifier", () => {
    expect(acceleratorFromKeydown(press("F6"))).toBe("F6");
    expect(acceleratorFromKeydown(press("F1"))).toBe("F1");
    expect(acceleratorFromKeydown(press("F24"))).toBe("F24");
    expect(acceleratorFromKeydown(press("F12", { shiftKey: true }))).toBe("Shift+F12");
  });

  test("a letter, a digit or Space alone would swallow typing everywhere: refused", () => {
    expect(acceleratorFromKeydown(press("KeyD"))).toBeNull();
    expect(acceleratorFromKeydown(press("Digit5"))).toBeNull();
    expect(acceleratorFromKeydown(press("Space"))).toBeNull();
  });

  test("a modifier by itself is ignored: the user is still on the way to the key", () => {
    for (const code of ["ControlLeft", "ControlRight", "AltLeft", "AltRight", "ShiftLeft", "ShiftRight", "MetaLeft", "MetaRight"]) {
      expect(acceleratorFromKeydown(press(code, { ctrlKey: true, altKey: true, shiftKey: true, metaKey: true })), code).toBeNull();
    }
  });

  test("letters, digits and the named keys", () => {
    expect(acceleratorFromKeydown(press("KeyA", { altKey: true }))).toBe("Alt+A");
    expect(acceleratorFromKeydown(press("KeyZ", { altKey: true }))).toBe("Alt+Z");
    expect(acceleratorFromKeydown(press("Digit0", { ctrlKey: true }))).toBe("Ctrl+0");
    expect(acceleratorFromKeydown(press("Enter", { ctrlKey: true }))).toBe("Ctrl+Enter");
    expect(acceleratorFromKeydown(press("PageUp", { altKey: true }))).toBe("Alt+PageUp");
    expect(acceleratorFromKeydown(press("ArrowLeft", { altKey: true }))).toBe("Alt+ArrowLeft");
    expect(acceleratorFromKeydown(press("Backspace", { ctrlKey: true }))).toBe("Ctrl+Backspace");
  });

  test("a key the shortcut plugin has no name for is refused", () => {
    expect(acceleratorFromKeydown(press("Escape", { ctrlKey: true }))).toBeNull();
    expect(acceleratorFromKeydown(press("F25", { ctrlKey: true }))).toBeNull();
    expect(acceleratorFromKeydown(press("F0", { ctrlKey: true }))).toBeNull();
    expect(acceleratorFromKeydown(press("IntlBackslash", { ctrlKey: true }))).toBeNull();
    expect(acceleratorFromKeydown(press("constructor", { ctrlKey: true }))).toBeNull();
    expect(acceleratorFromKeydown(press("", { ctrlKey: true }))).toBeNull();
  });
});
