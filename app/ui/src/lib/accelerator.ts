// The accelerator (the text the Rust side saves and the shortcut plugin reads, such as "Ctrl+Alt+Space") of a key press.
// The keys it can name are the ones the Rust side checks in `every_key_the_recorder_can_produce_is_one_the_plugin_reads`.

/** What a `keydown` event says about the key, and what it is pressed with. */
export interface KeyPress {
  code: string;
  ctrlKey: boolean;
  altKey: boolean;
  shiftKey: boolean;
  metaKey: boolean;
}

// By `code`, the key on the keyboard and not the letter it types, so that the shortcut is the same on every layout.
// A Map: a plain object would answer for "constructor" and the like.
const NAMED_KEYS = new Map(
  [
    "Space",
    "Enter",
    "Tab",
    "Backspace",
    "Delete",
    "Insert",
    "Home",
    "End",
    "PageUp",
    "PageDown",
    "ArrowUp",
    "ArrowDown",
    "ArrowLeft",
    "ArrowRight",
  ].map((name) => [name, name]),
);

const FUNCTION_KEY = /^F([1-9]|1\d|2[0-4])$/;

/** The name of the key in the accelerator; null for a modifier on its own and for keys that have no name there. */
function keyName(code: string): string | null {
  const letter = /^Key([A-Z])$/.exec(code);
  if (letter) return letter[1];
  const digit = /^Digit(\d)$/.exec(code);
  if (digit) return digit[1];
  if (FUNCTION_KEY.test(code)) return code;
  return NAMED_KEYS.get(code) ?? null;
}

/**
 * The accelerator for this key press, or null when there isn't one (yet): a modifier on its own, a key with no name,
 * or a key that needs a modifier and has none. Only the function keys stand alone: any other key as a shortcut
 * would take it away from typing in every app. The Command key is "Super", as the plugin spells it.
 */
export function acceleratorFromKeydown(press: KeyPress): string | null {
  const key = keyName(press.code);
  if (key === null) return null;
  const modifiers = [
    press.metaKey && "Super",
    press.ctrlKey && "Ctrl",
    press.altKey && "Alt",
    press.shiftKey && "Shift",
  ].filter((name) => name !== false);
  if (modifiers.length === 0 && !FUNCTION_KEY.test(key)) return null;
  return [...modifiers, key].join("+");
}
