import { afterEach, beforeEach, describe, expect, test, vi } from "vitest";
import type { MediaView } from "./api";
import { FakeMedia, fakeMedia } from "../test-helpers";
import { PlayerController, STALL_MS, TICK_MS } from "./player.svelte";
import type { FetchMedia } from "./player.svelte";

const ID = "2026-03-02 10-00-00";
/** The address of the server as it was, and as it is after the app started it again: another port and another token. */
const OLD = fakeMedia({ url: "http://127.0.0.1:41000/tok-a/1" });
const NEW = fakeMedia({ url: "http://127.0.0.1:52000/tok-b/1" });

let element: FakeMedia;
let fetchMedia: ReturnType<typeof vi.fn<FetchMedia>>;

/** A player on a recording whose address is `OLD`, with an element that is attached to it. `now` is the address the server gives from here on. */
function setup(now: MediaView | null = OLD) {
  fetchMedia = vi.fn<FetchMedia>(async () => now);
  const player = new PlayerController(fetchMedia);
  element = new FakeMedia();
  player.bind(ID, OLD);
  const detach = player.attach(element as unknown as HTMLMediaElement);
  return { player, detach };
}

/** Lets the calls that are waited for finish, with the clock where it is. */
const settle = () => vi.advanceTimersByTimeAsync(0);

/** A read of the address that the test answers when it chooses. */
function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => (resolve = done));
  return { promise, resolve };
}

beforeEach(() => {
  vi.useFakeTimers();
});

afterEach(() => {
  vi.useRealTimers();
});

describe("what the element plays", () => {
  test("an element that is attached plays the address of the recording, and a recording with no media has no player", () => {
    const { player } = setup();
    expect(element.src).toBe(OLD.url);
    expect(player.available).toBe(true);
    expect(player.video).toBe(true);

    player.bind("another", null);
    expect(player.available).toBe(false);
  });

  test("the position is read every half second, and whether it plays", () => {
    const { player } = setup();
    expect(TICK_MS).toBe(500);
    element.playAt(12.5);

    vi.advanceTimersByTime(TICK_MS - 1);
    expect(player.currentTime).toBe(0);
    vi.advanceTimersByTime(1);
    expect(player.currentTime).toBe(12.5);
    expect(player.playing).toBe(true);

    element.paused = true;
    vi.advanceTimersByTime(TICK_MS);
    expect(player.playing).toBe(false);
  });

  test("a detached element is no longer read", () => {
    const { player, detach } = setup();
    detach();
    element.playAt(30);
    vi.advanceTimersByTime(TICK_MS * 4);
    expect(player.currentTime).toBe(0);
  });

  test("another recording starts from its beginning, with a clean state", () => {
    const { player } = setup();
    element.playAt(80);
    vi.advanceTimersByTime(TICK_MS);
    element.emit("error");

    player.bind("another", fakeMedia({ url: "http://127.0.0.1:41000/tok-a/2" }));
    expect(player.currentTime).toBe(0);
    expect(player.failed).toBe(false);
    expect(element.src).toBe("http://127.0.0.1:41000/tok-a/2");
  });
});

describe("seek", () => {
  test("stamp_click_seeks_player: a seek goes to the exact second and plays", async () => {
    const { player } = setup();
    element.playAt(5);

    await player.seek(65);

    expect(element.currentTime).toBe(65);
    expect(player.currentTime).toBe(65);
    expect(element.play).toHaveBeenCalled();
    // The element was playing: no one asked the server for an address.
    expect(fetchMedia).not.toHaveBeenCalled();
  });

  test("after a pause the address is read again first; when it is the same the seek goes on in place", async () => {
    const { player } = setup();

    await player.seek(65);

    expect(fetchMedia).toHaveBeenCalledWith(ID);
    expect(element.sources).toEqual([OLD.url]);
    expect(element.currentTime).toBe(65);
    expect(element.play).toHaveBeenCalledTimes(1);
  });

  test("after a pause a new port is swapped in, and the seek lands on the new address once it is ready", async () => {
    const { player } = setup(NEW);

    await player.seek(65);

    expect(element.src).toBe(NEW.url);
    // The new address is loading: the position and the play wait for it.
    expect(element.play).not.toHaveBeenCalled();
    element.emit("loadedmetadata");
    expect(element.currentTime).toBe(65);
    expect(element.play).toHaveBeenCalledTimes(1);
    expect(player.failed).toBe(false);
  });

  test("a seek while a new address is loading waits for it, and the last seek is the one that counts", async () => {
    const { player } = setup(NEW);
    await player.seek(65);
    expect(element.src).toBe(NEW.url);

    // The new address has not loaded yet: the element is not asked for a position, which it would forget.
    element.paused = true;
    await player.seek(120);
    expect(element.currentTime).toBe(0);
    expect(element.play).not.toHaveBeenCalled();

    element.emit("loadedmetadata");
    expect(element.currentTime).toBe(120);
    expect(player.currentTime).toBe(120);
    expect(element.play).toHaveBeenCalledTimes(1);
  });

  test("a seek with no element does nothing", async () => {
    const player = new PlayerController(async () => OLD);
    player.bind(ID, OLD);
    await player.seek(10);
    expect(player.currentTime).toBe(0);
  });

  test("a seek whose server cannot be asked still seeks in place", async () => {
    const { player } = setup();
    fetchMedia.mockRejectedValue(new Error("gone"));

    await player.seek(30);

    expect(element.currentTime).toBe(30);
    expect(element.play).toHaveBeenCalled();
  });
});

describe("pause", () => {
  test("pause stops what a seek started, and the player says it is not playing", async () => {
    const { player } = setup();
    await player.seek(65);
    element.playAt(65);
    vi.advanceTimersByTime(TICK_MS);
    expect(player.playing).toBe(true);

    player.pause();

    expect(element.pause).toHaveBeenCalledTimes(1);
    expect(player.playing).toBe(false);
    expect(element.currentTime).toBe(65);
  });

  test("a pause while a new address is loading keeps it from playing when it has loaded", async () => {
    const { player } = setup(NEW);
    await player.seek(65);
    expect(element.src).toBe(NEW.url);

    player.pause();
    element.emit("loadedmetadata");

    // It goes to the second the seek asked for, and stays there.
    expect(element.currentTime).toBe(65);
    expect(element.play).not.toHaveBeenCalled();
  });

  test("a pause while a seek waits to look at the address cancels the play that the seek was to make", async () => {
    const { player } = setup();
    const answer = deferred<MediaView | null>();
    fetchMedia.mockImplementation(() => answer.promise);

    // The element is paused, so the seek asks the server first.
    const seeking = player.seek(65);
    await settle();
    player.pause();
    answer.resolve(OLD);
    await seeking;
    await settle();

    expect(element.play).not.toHaveBeenCalled();
    expect(element.currentTime).toBe(0);
    expect(player.playing).toBe(false);
  });

  test("a pause while a seek waits keeps a new address from being swapped in to play, and a seek after it plays as usual", async () => {
    const { player } = setup(NEW);
    const answer = deferred<MediaView | null>();
    fetchMedia.mockImplementationOnce(() => answer.promise);

    const seeking = player.seek(65);
    await settle();
    player.pause();
    answer.resolve(NEW);
    await seeking;

    expect(element.sources).toEqual([OLD.url]);

    // The pause belongs to the seek that was waiting: the next one is not cancelled by it.
    fetchMedia.mockImplementation(async () => NEW);
    await player.seek(70);
    expect(element.src).toBe(NEW.url);
    element.emit("loadedmetadata");
    expect(element.currentTime).toBe(70);
    expect(element.play).toHaveBeenCalledTimes(1);
  });

  test("a pause with no element, or after the player let go of the file, does nothing", () => {
    const bare = new PlayerController(async () => OLD);
    bare.bind(ID, OLD);
    expect(() => bare.pause()).not.toThrow();

    const { player } = setup();
    player.release();
    element.pause.mockClear();
    player.pause();
    expect(element.pause).not.toHaveBeenCalled();
  });
});

describe("play with the controls after a pause", () => {
  test("a new port is swapped in, at the same position, and it plays on", async () => {
    setup(NEW);
    element.currentTime = 42;

    element.paused = false;
    element.emit("play");
    await settle();

    expect(element.src).toBe(NEW.url);
    element.emit("loadedmetadata");
    expect(element.currentTime).toBe(42);
    expect(element.play).toHaveBeenCalledTimes(1);
  });

  test("the same address is left alone, and it is read once for each pause, not for each play", async () => {
    setup(OLD);

    element.paused = false;
    element.emit("play");
    await settle();
    element.emit("play");
    await settle();
    expect(fetchMedia).toHaveBeenCalledTimes(1);

    element.paused = true;
    element.emit("pause");
    element.paused = false;
    element.emit("play");
    await settle();
    expect(fetchMedia).toHaveBeenCalledTimes(2);
    expect(element.sources).toEqual([OLD.url]);
  });
});

describe("the element fails", () => {
  test("an error reads the address again, plays from the new one and goes back to where it was", async () => {
    const { player } = setup(NEW);
    element.playAt(42.5);

    element.emit("error");
    await settle();

    expect(fetchMedia).toHaveBeenCalledWith(ID);
    expect(element.src).toBe(NEW.url);
    expect(element.currentTime).toBe(0);
    element.emit("loadedmetadata");
    expect(element.currentTime).toBe(42.5);
    // It played, so it plays on.
    expect(element.play).toHaveBeenCalledTimes(1);
    expect(player.failed).toBe(false);
  });

  test("a recording that was paused stays paused after it is mended", async () => {
    setup(NEW);
    element.currentTime = 10;

    element.emit("error");
    await settle();
    element.emit("loadedmetadata");

    expect(element.currentTime).toBe(10);
    expect(element.play).not.toHaveBeenCalled();
  });

  test("an error with the same address is a file that cannot be played: the player says so and does not ask again", async () => {
    const { player } = setup(OLD);

    element.emit("error");
    await settle();
    expect(player.failed).toBe(true);
    expect(player.available).toBe(false);
    expect(fetchMedia).toHaveBeenCalledTimes(1);

    element.emit("error");
    await settle();
    expect(fetchMedia).toHaveBeenCalledTimes(1);
  });

  test("an error when the server gave no address, or cannot be asked, is a failure too", async () => {
    const first = setup(null);
    element.emit("error");
    await settle();
    expect(first.player.failed).toBe(true);

    const second = setup(OLD);
    fetchMedia.mockRejectedValue("gone");
    element.emit("error");
    await settle();
    expect(second.player.failed).toBe(true);
  });

  test("addresses that keep failing are not read for ever", async () => {
    const { player } = setup();
    let n = 0;
    fetchMedia.mockImplementation(async () => fakeMedia({ url: `http://127.0.0.1:${50000 + ++n}/tok/1` }));

    for (let i = 0; i < 10 && !player.failed; i += 1) {
      element.emit("error");
      await settle();
    }

    expect(player.failed).toBe(true);
    expect(fetchMedia.mock.calls.length).toBeLessThanOrEqual(4);
  });

  test("an address that works again forgets the failures before it", async () => {
    const { player } = setup();
    let n = 0;
    fetchMedia.mockImplementation(async () => fakeMedia({ url: `http://127.0.0.1:${50000 + ++n}/tok/1` }));

    for (let i = 0; i < 8; i += 1) {
      element.emit("error");
      await settle();
      element.emit("loadedmetadata");
    }

    expect(player.failed).toBe(false);
  });

  test("two errors at once make one read", async () => {
    setup(NEW);
    element.emit("error");
    element.emit("error");
    await settle();
    expect(fetchMedia).toHaveBeenCalledTimes(1);
  });

  test("an element that waits for data for 8 s reads the address again", async () => {
    setup(NEW);
    element.playAt(20);
    expect(STALL_MS).toBe(8000);

    element.emit("waiting");
    await vi.advanceTimersByTimeAsync(STALL_MS - 1);
    expect(fetchMedia).not.toHaveBeenCalled();
    await vi.advanceTimersByTimeAsync(1);

    expect(fetchMedia).toHaveBeenCalledTimes(1);
    expect(element.src).toBe(NEW.url);
    element.emit("loadedmetadata");
    expect(element.currentTime).toBe(20);
    expect(element.play).toHaveBeenCalledTimes(1);
  });

  test("a stalled element is mended the same way", async () => {
    setup(NEW);
    element.emit("stalled");
    await vi.advanceTimersByTimeAsync(STALL_MS);
    expect(element.src).toBe(NEW.url);
  });

  test("an element that is back before 8 s is left alone", async () => {
    setup(NEW);
    element.playAt(20);
    element.emit("waiting");
    await vi.advanceTimersByTimeAsync(STALL_MS - 100);
    element.emit("playing");
    await vi.advanceTimersByTimeAsync(STALL_MS * 2);
    expect(fetchMedia).not.toHaveBeenCalled();
  });

  test("a pause ends the wait: nobody watches a player that is not playing", async () => {
    setup(NEW);
    element.playAt(20);
    element.emit("waiting");
    element.pause();
    await vi.advanceTimersByTimeAsync(STALL_MS * 2);
    expect(fetchMedia).not.toHaveBeenCalled();
  });

  test("a wait of 8 s on an address that is still the right one changes nothing and is no failure", async () => {
    const { player } = setup(OLD);
    element.playAt(20);

    element.emit("waiting");
    await vi.advanceTimersByTimeAsync(STALL_MS);

    expect(fetchMedia).toHaveBeenCalledTimes(1);
    expect(element.sources).toEqual([OLD.url]);
    expect(player.failed).toBe(false);
  });
});

describe("a play and an error that come together", () => {
  /**
   * The element was paused and has an error as it is played, in either order. The reads are held, and the test says what each finds.
   * Which address the error is of is the one that was played then: it is not a failure when a read has since put another one in.
   */
  function together(first: "play" | "error") {
    const { player } = setup();
    const reads: ReturnType<typeof deferred<MediaView | null>>[] = [];
    fetchMedia.mockImplementation(() => {
      const read = deferred<MediaView | null>();
      reads.push(read);
      return read.promise;
    });
    element.currentTime = 42;
    const play = () => {
      element.paused = false;
      element.emit("play");
    };
    const fail = () => element.emit("error");
    if (first === "play") {
      play();
      fail();
    } else {
      fail();
      play();
    }
    return { player, reads };
  }

  test.each(["play", "error"] as const)(
    "%s first: a new address that is found is played from, once, and the error of the old one is no failure",
    async (first) => {
      const { player, reads } = together(first);
      // One read of the address at a time.
      expect(reads).toHaveLength(1);

      reads[0].resolve(NEW);
      await settle();

      expect(player.failed).toBe(false);
      expect(player.available).toBe(true);
      expect(element.sources).toEqual([OLD.url, NEW.url]);
      // The error is of the address that was just left: there is nothing to read for it.
      expect(reads).toHaveLength(1);
      element.emit("loadedmetadata");
      expect(element.currentTime).toBe(42);
      expect(element.play).toHaveBeenCalledTimes(1);
    },
  );

  test.each(["play", "error"] as const)(
    "%s first: when the address is still the right one the error is a failure: a file that can't be played",
    async (first) => {
      const { player, reads } = together(first);

      reads[0].resolve(OLD);
      await settle();
      if (first === "play") {
        // The read of the play found nothing wrong; the error that came meanwhile is read for now.
        expect(reads).toHaveLength(2);
        reads[1].resolve(OLD);
        await settle();
      }

      expect(reads).toHaveLength(first === "play" ? 2 : 1);
      expect(player.failed).toBe(true);
      expect(element.sources).toEqual([OLD.url]);
    },
  );

  test("an error that is read for after a wrong address was swapped in is the failure of that one", async () => {
    const { player, reads } = together("play");
    reads[0].resolve(NEW);
    await settle();
    element.emit("loadedmetadata");

    // The new address fails as well, and the server says it is the right one.
    fetchMedia.mockImplementation(async () => NEW);
    element.emit("error");
    await settle();

    expect(player.failed).toBe(true);
  });
});

describe("a new address that the window brings", () => {
  test("while an error is being read for, it is not failed by that error: the error was of the address before it", async () => {
    const { player } = setup();
    const read = deferred<MediaView | null>();
    fetchMedia.mockImplementation(() => read.promise);
    element.emit("error");

    // The window read the detail again meanwhile, and it has the new address.
    player.bind(ID, NEW);
    read.resolve(NEW);
    await settle();

    expect(player.failed).toBe(false);
    expect(player.available).toBe(true);
    expect(element.sources).toEqual([OLD.url, NEW.url]);
  });
});

describe("a second read while a swap is still loading", () => {
  test("a second recovery goes to where the first was going, not to 0", async () => {
    const { player } = setup();
    element.playAt(42);
    let n = 0;
    fetchMedia.mockImplementation(async () => fakeMedia({ url: `http://127.0.0.1:${50000 + ++n}/tok/1` }));

    element.emit("error");
    await settle();
    expect(element.src).toBe("http://127.0.0.1:50001/tok/1");
    // The first new address has not loaded: the element is at 0 and paused. It fails too.
    element.emit("error");
    await settle();
    expect(element.src).toBe("http://127.0.0.1:50002/tok/1");

    element.emit("loadedmetadata");
    expect(element.currentTime).toBe(42);
    expect(player.currentTime).toBe(42);
    expect(element.play).toHaveBeenCalledTimes(1);
  });

  test("a play of the controls that finds yet another address goes to where the seek was going", async () => {
    const { player } = setup(NEW);
    await player.seek(65);
    expect(element.src).toBe(NEW.url);

    // The controls play while that address is loading, and the server has moved again.
    const NEWER = fakeMedia({ url: "http://127.0.0.1:63000/tok-c/1" });
    fetchMedia.mockResolvedValue(NEWER);
    element.emit("pause");
    element.paused = false;
    element.emit("play");
    await settle();

    expect(element.src).toBe(NEWER.url);
    element.emit("loadedmetadata");
    expect(element.currentTime).toBe(65);
    expect(element.play).toHaveBeenCalledTimes(1);
  });
});

describe("a different address gives the player another chance", () => {
  test("after a failure a new address of the same recording clears it, and the player is back", async () => {
    const { player } = setup(OLD);
    element.emit("error");
    await settle();
    expect(player.failed).toBe(true);
    expect(player.available).toBe(false);

    player.bind(ID, NEW);

    expect(player.failed).toBe(false);
    expect(player.available).toBe(true);
    expect(element.src).toBe(NEW.url);
  });

  test("the count of the addresses that failed starts again with it", async () => {
    const { player } = setup();
    let n = 0;
    fetchMedia.mockImplementation(async () => fakeMedia({ url: `http://127.0.0.1:${50000 + ++n}/tok/1` }));
    for (let i = 0; i < 10 && !player.failed; i += 1) {
      element.emit("error");
      await settle();
    }
    expect(player.failed).toBe(true);

    player.bind(ID, fakeMedia({ url: "http://127.0.0.1:61000/tok/1" }));
    expect(player.failed).toBe(false);
    // A new run of addresses that fail has its whole count again: the next one is mended, not given up.
    element.emit("error");
    await settle();
    expect(player.failed).toBe(false);
    expect(element.src).toBe(`http://127.0.0.1:${50000 + n}/tok/1`);
  });

  test("the same address again clears nothing", async () => {
    const { player } = setup(OLD);
    element.emit("error");
    await settle();

    player.bind(ID, { ...OLD });

    expect(player.failed).toBe(true);
  });
});

describe("the address of the recording changes under the player", () => {
  test("the same recording with a new address plays from the new one, from where it was", () => {
    const { player } = setup();
    element.playAt(33);

    player.bind(ID, NEW);

    expect(element.src).toBe(NEW.url);
    element.emit("loadedmetadata");
    expect(element.currentTime).toBe(33);
    expect(element.play).toHaveBeenCalledTimes(1);
  });

  test("the same address again changes nothing", () => {
    const { player } = setup();
    element.playAt(33);

    player.bind(ID, { ...OLD });

    expect(element.sources).toEqual([OLD.url]);
  });
});

describe("release: what Windows needs before it moves a folder to the Trash", () => {
  test("the player stops, lets go of its file and loads nothing, in that order", () => {
    const { player } = setup();
    element.playAt(33);

    player.release();

    expect(element.calls).toEqual(["pause", "removeAttribute(src)", "load"]);
    expect(element.getAttribute("src")).toBeNull();
    expect(player.available).toBe(false);
  });

  test("a released player does not read, mend or listen any more", async () => {
    const { player } = setup(NEW);
    element.playAt(33);
    element.emit("waiting");

    player.release();
    element.emit("error");
    await vi.advanceTimersByTimeAsync(STALL_MS * 2);

    expect(fetchMedia).not.toHaveBeenCalled();
    expect(player.failed).toBe(false);
  });

  test("what release gives back puts the player back as it was, at the position it had, paused", () => {
    const { player, detach } = setup();
    element.playAt(33);
    vi.advanceTimersByTime(TICK_MS);

    const restore = player.release();
    detach();
    expect(player.available).toBe(false);

    restore();
    expect(player.available).toBe(true);
    const again = new FakeMedia();
    player.attach(again as unknown as HTMLMediaElement);
    expect(again.src).toBe(OLD.url);
    again.emit("loadedmetadata");
    expect(again.currentTime).toBe(33);
    expect(again.play).not.toHaveBeenCalled();
  });

  test("a restore that comes after the player has moved on to another recording does nothing", () => {
    const { player } = setup();
    const restore = player.release();

    player.bind("another", fakeMedia({ url: "http://127.0.0.1:41000/tok-a/2" }));
    restore();

    expect(player.url).toBe("http://127.0.0.1:41000/tok-a/2");
  });

  test("when the recording stays, its address brings the player back, at the position it had", () => {
    const { player, detach } = setup();
    element.playAt(33);
    vi.advanceTimersByTime(TICK_MS);

    player.release();
    detach();
    expect(player.available).toBe(false);

    player.bind(ID, OLD);
    expect(player.available).toBe(true);
    const again = new FakeMedia();
    player.attach(again as unknown as HTMLMediaElement);
    expect(again.src).toBe(OLD.url);
    again.emit("loadedmetadata");
    expect(again.currentTime).toBe(33);
    // It does not start on its own.
    expect(again.play).not.toHaveBeenCalled();
  });
});
