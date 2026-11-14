## What changes

<!-- What this pull request changes and why. Link the issue it fixes, e.g. "Fixes #12". -->

## Testing

<!-- How you checked it: cargo test, pnpm --dir app/ui test, steps in the app, the system you tried it on… -->

## Checklist

- [ ] Commit messages follow [Conventional Commits](https://www.conventionalcommits.org) (`feat:`, `fix:`, `docs:`…), and so does the title; no AI attribution trailers
- [ ] `cargo fmt --all --check`, `cargo clippy --all-targets --locked -- -D warnings` and `cargo test --all --locked` pass
- [ ] `pnpm --dir app/ui check` and `pnpm --dir app/ui test` pass
- [ ] New or changed interface strings are in both `app/ui/src/locales/en.json` and `app/ui/src/locales/vi.json`
- [ ] Changes users can see are described in `README.md` and `README.vi.md`
