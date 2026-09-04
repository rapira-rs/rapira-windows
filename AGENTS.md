# Repository instructions

## All text

- Write all English with ASD-STE100 Simplified Technical English.
- Use short sentences, active voice, approved vocabulary, and one instruction per sentence.
- Use literal terms. Avoid idioms, slang, metaphors, decorative text, and unnecessary synonyms.
- Keep one paragraph or bullet on one line. Do not hard-wrap prose.
- Do not use em dashes or en dashes.
- Describe the current design. Do not add migration or deprecation text before 1.0.

## Other

- Review a defect against realistic use. Document a case caused only by misuse or an implausible condition. Do not add complex code for that case.

## Settled design

- Support ZTS PHP 8.4 and 8.5 only. `build.rs` rejects NTS builds.
- Support Windows 10, Windows 11, and Windows Server on x64. Support Windows 11 on ARM64.
- Build and test x64 and ARM64 on matching native CI runners. Use native architecture tools for local work.
- Build release PHP from official PHP 8.4 and 8.5 source with `ci/build-php.ps1`. Bundle the matching project-built runtime with each release archive.
- Run one process with a static pool of PHP interpreter threads.
- Run MINIT once before the interpreter threads start.
- Run in the foreground. Keep pidfile support.
- `pool.processes` and `--processes` set the interpreter thread count.
- Do not add reload, status, or dynamic pool scaling.
- Use `TerminateProcess` for a forced exit while interpreter threads can be alive.
- Put host logic in Rust through exported Zend APIs when practical. Use C for argument parsing shells, bailout isolation, and macro shims.
- Do not preserve backward compatibility before 1.0.

## Git

- Never commit or push to `main`. Never force-push, reset, or rewrite published history.
- Do not merge, close, or reopen pull requests or change repository settings unless requested.
- Do not add `Co-authored-by` trailers or AI attribution to commits, pull request descriptions, or comments.
- Keep pull request descriptions short. Include only significant changes. Do not add verification sections or test-run narration.

## Comments

- Explain what the code does or why a constraint exists. Do not restate the code.
- Add the authoritative documentation link for a non-obvious external term. Verify the URL and anchor.
- Do not write numbered-step comments.
- State only the current constraint. Do not compare it with a rejected or old design.
- Use `//` comments in hand-written `.c` and `.h` files. Do not use block comments.
- Generate each `*_arginfo.h` file from its `.stub.php` file. Do not edit generated headers directly.
- Use capitals only for identifiers, acronyms, and constants.
- Keep the intentional `Rustttt` and `trust me, I'm a developer` joke comments.

## Tests

- Put unit tests in their crate under `#[cfg(test)]`.
- Put integration tests in `crates/tests`.
- Put end-to-end tests in `crates/tests/tests/e2e/` behind the `e2e` feature so a workspace test run skips them.
- Derive expected values from the applicable specification, php-src, or the decided requirement before reading the implementation. Do not backfill an assertion from observed output.
- Test edge cases, configuration precedence, derived values, and validation failures. Do not add trivial tests.
- Use worker or dispatcher mode for new tests.
- Check PHP behavior against php-src or a small PHP script.

Use `dev.ps1` for the standard tasks: `build`, `test`, `test_e2e`, `coverage`, `stubs`, and `clangd`. The `clangd` task writes its compilation database below ignored `target/clangd`.

## Dependencies

Use mainstream crates with broad adoption. Reject niche or single-maintainer crates. Prefer direct platform APIs to unnecessary wrappers. Record the reason for a new dependency in the pull request description.

## Dead code

Delete dead code and defenses for cases that cannot occur. Existing code and tests do not justify unused behavior.

## Docs

- Apply the rules in "All text" to documentation.
- Describe only the current design before 1.0. Do not add migration tables or deprecation notes.
- Keep one paragraph or bullet on one line. Do not hard-wrap prose.
- Verify an RFC section number and its text before you cite it. Link to the exact section.

## Known false positives

- Cargo is authoritative when rust-analyzer reports `E0277: Arguments<'_>: Sync` for `Box::pin` over a `tokio::select!` and `cargo check` succeeds.
- `--enable-opcache` is valid for PHP 8.4 and invalid for PHP 8.5. Keep the version filter in `ci/build-php.ps1`.
- Extension availability differs between PHP release archives. Keep the `extension_loaded` guards in tests.
- `.clang-tidy` runs in survey mode. Zend macro signatures can trigger `bugprone-*` findings. CI does not run this check.

## Reviews

- Validate each automated finding against the code. A confident statement is not evidence.
- Review the underlying case and the final behavior.
- Prefer the smallest safe fix. Avoid speculative hardening and unrelated cleanup.
- Include relevant automated findings in the review and resolve their threads. Do not reply to automated comments.
- Check whether the changed code or related existing code can be simpler without changing behavior.
