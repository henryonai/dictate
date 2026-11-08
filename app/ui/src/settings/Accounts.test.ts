import { render, screen, within } from "@testing-library/svelte";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { api } from "../lib/api";
import { setLanguage } from "../lib/i18n";
import { fakeView, loadedStore, stubApi } from "../test-helpers";
import Accounts from "./Accounts.svelte";

beforeEach(() => {
  stubApi();
});

afterEach(() => {
  vi.restoreAllMocks();
  setLanguage("en");
});

const lastSaved = () => vi.mocked(api.saveSettings).mock.lastCall?.[0];

/** The AI part of the tab and the transcription part. */
async function sections() {
  return {
    ai: within(await screen.findByRole("region", { name: "AI — editing & summaries" })),
    stt: within(await screen.findByRole("region", { name: "Transcription API" })),
  };
}

function later<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

// ---- the check buttons

test("check button shows error text", async () => {
  const user = userEvent.setup();
  vi.mocked(api.checkAi).mockRejectedValue("API error 401: bad key");
  render(Accounts, { store: await loadedStore({ has_ai_key: true }) });
  const { ai } = await sections();

  await user.click(ai.getByRole("button", { name: "Check" }));

  expect(await ai.findByText("API error 401: bad key")).toBeInTheDocument();
});

test("an error from the Rust side is shown as it is, even when it is an Error", async () => {
  const user = userEvent.setup();
  vi.mocked(api.checkTranscribe).mockRejectedValue(new Error("connection refused"));
  render(Accounts, { store: await loadedStore() });
  const { stt } = await sections();

  await user.click(stt.getByRole("button", { name: "Check" }));

  expect(await stt.findByText("connection refused")).toBeInTheDocument();
});

test("the AI check says Checking… while it runs and then shows a success text in the UI language", async () => {
  const user = userEvent.setup();
  const check = later<string>();
  vi.mocked(api.checkAi).mockReturnValue(check.promise);
  render(Accounts, { store: await loadedStore({ has_ai_key: true, ai_model: "claude-haiku-5-5" }) });
  const { ai } = await sections();

  await user.click(ai.getByRole("button", { name: "Check" }));
  expect(await ai.findByRole("button", { name: "Checking…" })).toBeDisabled();

  // The Rust side's text is English; the UI says it in its own words.
  check.resolve("Connected: claude-haiku-5-5 answered.");
  expect(await ai.findByText("Connected: claude-haiku-5-5 answered.")).toBeInTheDocument();
  expect(ai.getByRole("button", { name: "Check" })).toBeEnabled();
});

test("the success text is Vietnamese in a Vietnamese UI", async () => {
  const user = userEvent.setup();
  setLanguage("vi");
  render(Accounts, { store: await loadedStore({ has_ai_key: true, ai_model: "claude-haiku-5-5", ui_language: "vi" }) });
  const ai = within(await screen.findByRole("region", { name: "AI — sửa chữ & tóm tắt" }));

  await user.click(ai.getByRole("button", { name: "Kiểm tra" }));

  expect(await ai.findByText("Đã kết nối: claude-haiku-5-5 đã trả lời.")).toBeInTheDocument();
});

test("the transcription check shows a success text with the model", async () => {
  const user = userEvent.setup();
  render(Accounts, { store: await loadedStore({ transcribe_model: "whisper-1" }) });
  const { stt } = await sections();

  await user.click(stt.getByRole("button", { name: "Check" }));

  expect(await stt.findByText("Connected: whisper-1 accepted the test audio.")).toBeInTheDocument();
});

test("a new check clears the result of the last one", async () => {
  const user = userEvent.setup();
  vi.mocked(api.checkAi).mockRejectedValueOnce("API error 401: bad key");
  render(Accounts, { store: await loadedStore({ has_ai_key: true }) });
  const { ai } = await sections();
  await user.click(ai.getByRole("button", { name: "Check" }));
  await ai.findByText("API error 401: bad key");

  vi.mocked(api.checkAi).mockReturnValue(later<string>().promise);
  await user.click(ai.getByRole("button", { name: "Check" }));

  expect(ai.queryByText("API error 401: bad key")).not.toBeInTheDocument();
});

test("a key typed but not saved yet is saved before it is checked", async () => {
  const user = userEvent.setup();
  const order: string[] = [];
  vi.mocked(api.setSecret).mockImplementation(async () => {
    order.push("save");
    return fakeView({ has_ai_key: true });
  });
  vi.mocked(api.checkAi).mockImplementation(async () => {
    order.push("check");
    return "Connected";
  });
  render(Accounts, { store: await loadedStore() });
  const { ai } = await sections();

  await user.type(ai.getByLabelText("API key"), "sk-typed-key");
  await user.click(ai.getByRole("button", { name: "Check" }));

  await vi.waitFor(() => expect(order).toEqual(["save", "check"]));
  expect(api.setSecret).toHaveBeenCalledWith("ai", "sk-typed-key");
  expect(ai.getByLabelText("API key")).toHaveValue("");
  // Saving the key was part of the check: its result stays.
  expect(await ai.findByText("Connected: claude-haiku-5-5 answered.")).toBeInTheDocument();
});

// ---- a result belongs to the settings it was got with

const aiResult = "API error 401: bad key";

async function aiWithResult(overrides = {}) {
  const user = userEvent.setup();
  vi.mocked(api.checkAi).mockRejectedValue(aiResult);
  render(Accounts, { store: await loadedStore({ has_ai_key: true, ...overrides }) });
  const parts = await sections();
  await user.click(parts.ai.getByRole("button", { name: "Check" }));
  await parts.ai.findByText(aiResult);
  return { user, ...parts };
}

test("the AI result goes away when the model changes", async () => {
  const { user, ai } = await aiWithResult();

  await user.clear(ai.getByLabelText("Model"));
  await user.type(ai.getByLabelText("Model"), "claude-sonnet-5");
  await user.tab();

  expect(ai.queryByText(aiResult)).not.toBeInTheDocument();
});

test("the AI result goes away when the base URL changes", async () => {
  const { user, ai } = await aiWithResult();

  await user.type(ai.getByLabelText("Base URL"), "/v2");
  await user.tab();

  expect(ai.queryByText(aiResult)).not.toBeInTheDocument();
});

test("the AI result goes away when the provider changes", async () => {
  const { user, ai } = await aiWithResult();
  vi.mocked(api.setAiProvider).mockResolvedValue(
    fakeView({ ai_provider: "openai", ai_base_url: "http://localhost:8317/v1", has_ai_key: true }),
  );

  await user.selectOptions(ai.getByLabelText("Provider"), "OpenAI-compatible");

  expect(ai.queryByText(aiResult)).not.toBeInTheDocument();
});

test("the AI result goes away when a new key is typed", async () => {
  const { user, ai } = await aiWithResult();

  await user.type(ai.getByLabelText("API key"), "sk-new");

  expect(ai.queryByText(aiResult)).not.toBeInTheDocument();
});

test("the AI result goes away when the key is removed", async () => {
  const { user, ai } = await aiWithResult();
  vi.mocked(api.setSecret).mockResolvedValue(fakeView({ has_ai_key: false }));

  await user.click(ai.getByRole("button", { name: "Remove key" }));

  await vi.waitFor(() => expect(ai.queryByText(aiResult)).not.toBeInTheDocument());
});

test("the AI result stays when only the transcription settings change", async () => {
  const { user, stt } = await aiWithResult();
  const { ai } = await sections();
  vi.mocked(api.setTranscribePreset).mockResolvedValue(fakeView({ transcribe_preset: "groq", has_ai_key: true }));

  await user.selectOptions(stt.getByLabelText("Service"), "Groq");
  await vi.waitFor(() => expect(stt.getByLabelText("Service")).toHaveDisplayValue("Groq"));

  expect(ai.getByText(aiResult)).toBeInTheDocument();
});

test("the transcription result goes away when the service or its model changes", async () => {
  const user = userEvent.setup();
  vi.mocked(api.checkTranscribe).mockRejectedValue("API error 401: bad key");
  render(Accounts, { store: await loadedStore() });
  const { stt } = await sections();
  await user.click(stt.getByRole("button", { name: "Check" }));
  await stt.findByText("API error 401: bad key");

  await user.selectOptions(await stt.findByLabelText("File model"), "gpt-4o-transcribe");
  expect(stt.queryByText("API error 401: bad key")).not.toBeInTheDocument();

  await user.click(stt.getByRole("button", { name: "Check" }));
  await stt.findByText("API error 401: bad key");
  vi.mocked(api.setTranscribePreset).mockResolvedValue(fakeView({ transcribe_preset: "groq" }));
  await user.selectOptions(stt.getByLabelText("Service"), "Groq");
  await vi.waitFor(() => expect(stt.queryByText("API error 401: bad key")).not.toBeInTheDocument());
});

// ---- the keys

test("key field never prefilled", async () => {
  render(Accounts, { store: await loadedStore({ has_ai_key: true, has_transcribe_key: true }) });
  const { ai, stt } = await sections();

  expect(ai.getByLabelText("API key")).toHaveValue("");
  expect(stt.getByLabelText("API key")).toHaveValue("");
  expect(ai.getByText("Saved")).toBeInTheDocument();
  expect(stt.getByText("Saved")).toBeInTheDocument();
});

test("the key field hides what is typed", async () => {
  render(Accounts, { store: await loadedStore() });
  const { ai } = await sections();
  expect(ai.getByLabelText("API key")).toHaveAttribute("type", "password");
});

test("without a key it says so, and there is nothing to remove", async () => {
  render(Accounts, { store: await loadedStore() });
  const { ai } = await sections();

  expect(ai.queryByText("Saved")).not.toBeInTheDocument();
  expect(ai.getByText("No key saved")).toBeInTheDocument();
  expect(ai.queryByRole("button", { name: "Remove key" })).not.toBeInTheDocument();
});

test("Save key stores the key and empties the field", async () => {
  const user = userEvent.setup();
  vi.mocked(api.setSecret).mockResolvedValue(fakeView({ has_ai_key: true }));
  render(Accounts, { store: await loadedStore() });
  const { ai } = await sections();
  expect(ai.getByRole("button", { name: "Save key" })).toBeDisabled();

  await user.type(ai.getByLabelText("API key"), "sk-secret-1");
  await user.click(ai.getByRole("button", { name: "Save key" }));

  expect(api.setSecret).toHaveBeenCalledWith("ai", "sk-secret-1");
  expect(await ai.findByText("Saved")).toBeInTheDocument();
  expect(ai.getByLabelText("API key")).toHaveValue("");
  expect(document.body.innerHTML).not.toContain("sk-secret-1");
});

test("Enter in the key field saves it too", async () => {
  const user = userEvent.setup();
  render(Accounts, { store: await loadedStore() });
  const { stt } = await sections();

  await user.type(stt.getByLabelText("API key"), "gsk-1{Enter}");

  expect(api.setSecret).toHaveBeenCalledWith("transcribe", "gsk-1");
});

test("Remove key sends an empty key, which deletes it", async () => {
  const user = userEvent.setup();
  vi.mocked(api.setSecret).mockResolvedValue(fakeView({ has_ai_key: false }));
  render(Accounts, { store: await loadedStore({ has_ai_key: true }) });
  const { ai } = await sections();

  await user.click(ai.getByRole("button", { name: "Remove key" }));

  expect(api.setSecret).toHaveBeenCalledWith("ai", "");
  expect(await ai.findByText("No key saved")).toBeInTheDocument();
});

test("the OpenRouter key of the AI part is shown as shared when the service has none of its own", async () => {
  render(Accounts, {
    store: await loadedStore({ transcribe_preset: "openrouter", has_ai_key: true, shares_openrouter_key: true }),
  });
  const { stt } = await sections();
  expect(stt.getByText("Sharing the OpenRouter key from the AI section")).toBeInTheDocument();
});

test("no sharing note when the service has its own key", async () => {
  render(Accounts, { store: await loadedStore({ transcribe_preset: "openrouter", has_transcribe_key: true }) });
  const { stt } = await sections();
  expect(stt.queryByText("Sharing the OpenRouter key from the AI section")).not.toBeInTheDocument();
});

test("the hint on the key says that the AI key serves only when the AI part is OpenRouter too", async () => {
  render(Accounts, { store: await loadedStore({ transcribe_preset: "openrouter" }) });
  const { stt } = await sections();
  expect(
    stt.getByText(
      "OpenRouter can do without its own key when the AI section uses OpenRouter too: it then uses the AI key above.",
    ),
  ).toBeInTheDocument();
});

// ---- the AI provider

test("the provider and its settings are shown", async () => {
  render(Accounts, { store: await loadedStore() });
  const { ai } = await sections();
  expect(ai.getByLabelText("Provider")).toHaveDisplayValue("Anthropic");
  expect(ai.getByLabelText("Base URL")).toHaveValue("https://api.anthropic.com");
  expect(ai.getByLabelText("Model")).toHaveValue("claude-haiku-5-5");
});

test("another provider is switched by the Rust side, which moves the base URL along", async () => {
  const user = userEvent.setup();
  vi.mocked(api.setAiProvider).mockResolvedValue(
    fakeView({ ai_provider: "openai", ai_base_url: "http://localhost:8317/v1" }),
  );
  render(Accounts, { store: await loadedStore() });
  const { ai } = await sections();

  await user.selectOptions(ai.getByLabelText("Provider"), "OpenAI-compatible");

  expect(api.setAiProvider).toHaveBeenCalledWith("openai");
  await vi.waitFor(() => expect(ai.getByLabelText("Base URL")).toHaveValue("http://localhost:8317/v1"));
});

test("the base URL and model save when the field is left", async () => {
  const user = userEvent.setup();
  render(Accounts, { store: await loadedStore() });
  const { ai } = await sections();

  await user.clear(ai.getByLabelText("Model"));
  await user.type(ai.getByLabelText("Model"), "claude-sonnet-5");
  await user.tab();

  expect(lastSaved()).toMatchObject({ ai_model: "claude-sonnet-5" });
});

// ---- the transcription service

test("another service is switched by the Rust side, which brings its base URL and models", async () => {
  const user = userEvent.setup();
  vi.mocked(api.setTranscribePreset).mockResolvedValue(
    fakeView({
      transcribe_preset: "groq",
      transcribe_base_url: "https://api.groq.com/openai/v1",
      transcribe_model: "whisper-large-v3-turbo",
      dictation_model: "whisper-large-v3-turbo",
    }),
  );
  render(Accounts, { store: await loadedStore() });
  const { stt } = await sections();

  await user.selectOptions(stt.getByLabelText("Service"), "Groq");

  expect(api.setTranscribePreset).toHaveBeenCalledWith("groq");
  await vi.waitFor(() => expect(stt.getByLabelText("Base URL")).toHaveValue("https://api.groq.com/openai/v1"));
  expect(stt.getByLabelText("File model")).toHaveDisplayValue("whisper-large-v3-turbo");
});

test("the models of the service are a dropdown, and the dictation and file models are picked apart", async () => {
  const user = userEvent.setup();
  render(Accounts, { store: await loadedStore() });
  const { stt } = await sections();

  const fileModel = await stt.findByLabelText("File model");
  const options = [...fileModel.querySelectorAll("option")].map((option) => option.textContent);
  expect(options).toEqual(["whisper-1", "gpt-4o-transcribe", "Other…"]);

  await user.selectOptions(stt.getByLabelText("Dictation model"), "gpt-4o-transcribe");
  expect(lastSaved()).toMatchObject({ dictation_model: "gpt-4o-transcribe", transcribe_model: "whisper-1" });

  await user.selectOptions(fileModel, "gpt-4o-transcribe");
  expect(lastSaved()).toMatchObject({ dictation_model: "gpt-4o-transcribe", transcribe_model: "gpt-4o-transcribe" });
});

test("any other model name can be typed", async () => {
  const user = userEvent.setup();
  render(Accounts, { store: await loadedStore() });
  const { stt } = await sections();

  await user.selectOptions(await stt.findByLabelText("File model"), "Other…");
  await user.type(stt.getByLabelText("File model name"), "my-whisper");
  await user.tab();

  expect(lastSaved()).toMatchObject({ transcribe_model: "my-whisper" });
});

test("a custom service has no list: the model is typed", async () => {
  render(Accounts, {
    store: await loadedStore({ transcribe_preset: "custom", transcribe_model: "my-model", dictation_model: "my-fast-model" }),
  });
  const { stt } = await sections();

  expect(await stt.findByLabelText("File model name")).toHaveValue("my-model");
  expect(stt.getByLabelText("Dictation model name")).toHaveValue("my-fast-model");
});

// ---- Get a key

test("Get a key opens the service's page in the system browser", async () => {
  const user = userEvent.setup();
  render(Accounts, { store: await loadedStore({ transcribe_preset: "groq" }) });
  const { stt } = await sections();

  const link = await stt.findByRole("link", { name: "Get a key" });
  expect(link).toHaveAttribute("href", "https://console.groq.com/keys");
  await user.click(link);

  expect(api.openUrl).toHaveBeenCalledWith("https://console.groq.com/keys");
});

test("Get a key doesn't take the app's own window to another page", async () => {
  render(Accounts, { store: await loadedStore() });
  const { stt } = await sections();
  const link = await stt.findByRole("link", { name: "Get a key" });

  const click = new MouseEvent("click", { bubbles: true, cancelable: true });
  link.dispatchEvent(click);

  expect(click.defaultPrevented).toBe(true);
});

test("a custom service has no page to get a key from", async () => {
  render(Accounts, { store: await loadedStore({ transcribe_preset: "custom" }) });
  const { stt } = await sections();
  await stt.findByLabelText("File model name");
  expect(stt.queryByRole("link", { name: "Get a key" })).not.toBeInTheDocument();
});

test("says so when the page can't be opened", async () => {
  const user = userEvent.setup();
  vi.mocked(api.openUrl).mockRejectedValue("not allowed: https://openrouter.ai/keys");
  render(Accounts, { store: await loadedStore({ transcribe_preset: "openrouter" }) });
  const { stt } = await sections();

  await user.click(await stt.findByRole("link", { name: "Get a key" }));

  expect(await stt.findByText("not allowed: https://openrouter.ai/keys")).toBeInTheDocument();
});
