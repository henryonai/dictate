# Security policy

## Supported versions

Only the [latest release](https://github.com/henryonai/dictate/releases/latest) gets security fixes. Please check that the problem still happens there before reporting it.

## Reporting a vulnerability

Please don't open a public issue. Email [henryhoang.tech@gmail.com](mailto:henryhoang.tech@gmail.com) with:

- what the problem is and what someone could do with it;
- the steps to reproduce it, or a proof of concept;
- your Dictate version, your operating system and how you installed Dictate (the `.dmg`, the `.exe`, the `.deb` or a build from source).

You'll get a reply confirming the report, and news as the fix progresses. Once a fixed release is out, you're credited in the release notes unless you'd rather not be named.

## What counts

Dictate records the microphone and the screen, holds API keys and can type into other apps, so these matter most:

- API keys ending up outside the credential store of the system (Keychain on macOS, Credential Manager on Windows, Secret Service on Linux), such as in files, logs, command lines or URLs;
- recordings, transcripts or dictated text sent anywhere other than the services the user set up: audio leaves the computer only when the user has chosen a cloud transcription API, and only the text of a transcript goes to the AI provider;
- the `dictate-mcp` server reading files outside the meetings library, or changing anything;
- the small local server that plays recordings in the Meetings window (it listens on `127.0.0.1` only, and its address has a random token) giving a file to anyone who does not have that token, or being reachable from another computer;
- the models that Dictate downloads (every model is checked by its size, and the speaker separation models by their SHA-256 as well, before it is used) being accepted without that check;
- a command that Dictate runs (ffmpeg, yt-dlp, `dictate --toggle`, `dictate --record`) being made to run something the user did not ask for;
- text written into a field the user didn't dictate into.

Problems in the services Dictate calls (OpenRouter, Anthropic, OpenAI, Groq…) should go to those services, and problems in FFmpeg or another library that Dictate includes should go to its own project. Please say if the way Dictate uses it is what makes it a problem.

## Unsigned builds

The installers aren't signed or notarized, because the project has no signing certificate yet: macOS and Windows warn the first time you open one, which is known and described in the [README](README.md#unsigned-builds). Each installer on a release has a `.sha256` file next to it, and the README shows how to [check the download](README.md#verify-the-download). Installers are built by GitHub Actions from the source of the release. A way for someone to get a different file than the one on the release page, or to get past that check, is worth reporting.
