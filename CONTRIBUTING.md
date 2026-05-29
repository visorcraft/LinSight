<!-- SPDX-FileCopyrightText: 2026 VisorCraft LLC -->
<!-- SPDX-License-Identifier: GPL-3.0-only -->

# Contributing to LinSight

Thanks for your interest. LinSight accepts:

1. Bug reports and feature requests via GitHub Issues.
2. Pull requests for code, docs, translations, and packaging.
3. Hardware plugins — see [`docs/plugin-sdk.md`](docs/plugin-sdk.md);
   reference implementation at
   [`examples/echo-plugin/`](examples/echo-plugin/).

## Dev loop

```bash
just ci   # must be green before opening a PR (117 tests at HEAD)
```

`just ci` runs fmt-check, clippy with `-D warnings`, and
`cargo test --workspace`. There's no hosted CI yet, so this is the
only automatic gate — please run it locally before sending a PR.

## Commit messages

Conventional Commits: `feat:`, `fix:`, `chore:`, `docs:`,
`refactor:`, `test:`, `perf:`.

A few rules the audit-driven hardening sprint surfaced as worth
making explicit:

- **One commit, one logical change.** If your subject line uses
  "and", "+", "plus", or a semicolon joining two scopes, split it.
  A focused PR is easier to review and easier to revert if it
  regresses something.
- **`revert:` means restore the prior state.** Don't sneak new
  code into a revert commit; that breaks `git bisect` and
  `git blame` indefinitely. If a revert also needs a forward fix,
  send the fix as a separate commit.
- **Don't ship speculative upstream-bug docs with code that's
  meant to work around them.** Verify with the compositor-bypass
  screenshot tool (or whatever your bypass channel is) before
  writing the bug report. This caught us once; see the
  "Misdiagnoses" block in
  [`docs/superpowers/plans/2026-05-25-v0.3.0-followups-completion-notes.md`](docs/superpowers/plans/2026-05-25-v0.3.0-followups-completion-notes.md).
- **"Done" means tested.** A network-facing binary or a public
  ABI surface with no test coverage is not done regardless of how
  clean the code looks. The dynamic-load test for
  `export_plugin!` and the rcgen-based mTLS smoke test are
  examples of the rigor expected here.

## License

By contributing, you agree your contribution is licensed under
GPL-3.0-only.
