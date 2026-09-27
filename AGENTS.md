# Repository instructions

## All text

- Write English with ASD-STE100 Simplified Technical English.
- Use short sentences, active voice, and literal terms.
- Keep one paragraph or bullet on one line. Do not hard-wrap prose.
- Do not use em dashes or en dashes.
- Describe the current design. Do not add migration or deprecation text before 1.0.

## Design and tests

Read `.claude/CLAUDE.md` for the settled architecture, PHP contract, and test policy. These instructions also apply to agents that do not use Claude.

- Support ZTS PHP 8.4 and 8.5 only.
- Build and test x64 and ARM64 on matching native runners. Use native tools for local work.
- Build release PHP from official source with `ci/build-php.ps1`. Bundle the matching project-built runtime.
- Run one process with a fixed interpreter thread pool and work queue for each plugin. `<plugin>.pool.processes` sets its thread count.
- Run MINIT once. Run module teardown on the boot thread after every interpreter has stopped.
- Run in the foreground. Keep pidfile support. Use console control events for shutdown.
- Do not add reload, status, or dynamic pool scaling.
- Use `TerminateProcess` for a forced exit while interpreter threads can be alive.
- Put host logic in Rust through exported Zend APIs when practical. Use C for argument parsing shells, bailout isolation, and macro shims.

## Git

- Never commit or push to `main`. Never force-push, reset, or rewrite published history.
- Do not merge, close, or reopen pull requests or change repository settings unless requested.
- Do not add `Co-authored-by` trailers or AI attribution.
- Keep pull request descriptions short. Include only significant changes. Do not add test-run narration.

## Comments

- Explain what the code does or why a constraint exists. Do not restate the code.
- Add the authoritative documentation link for a non-obvious external term. Verify the URL and anchor.
- Use `//` comments in hand-written `.c` and `.h` files.
- Generate each `*_arginfo.h` from its `.stub.php`. Do not edit generated headers.
- Keep the intentional `Rustttt` and `trust me, I'm a developer` comments.

## Dependencies and dead code

- Prefer `windows-sys` platform APIs to unnecessary wrappers.
- Record the reason for a new dependency in the pull request description.
- Delete dead code and defenses for cases that cannot occur. Existing tests do not justify unused behavior.
- Review defects against realistic use. Do not add complex code for cases caused only by misuse or implausible conditions.

## Reviews

- Validate each automated finding against the code. A confident statement is not evidence.
- Review the underlying case and the final behavior.
- Prefer the smallest safe fix. Avoid speculative hardening and unrelated cleanup.
- Include relevant automated findings in the review and resolve their threads. Do not reply to automated comments.
- Check whether the changed code can be simpler without changing behavior.
