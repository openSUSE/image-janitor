# Rules for AGENTS

## Dev environment tips
- When working on a feature, Use red/green TDD. First create a git commit with the new testcase then do the code changes in a separate git commit.
- Never try to do git push

## Testing instructions
- From the package root you can just call `cargo test`. The commit should pass all tests before you commit anything
- Fix any test or type errors until the whole suite is green.
- Add or update tests for the code you change, even if nobody asked.
- Before tagging a new release, ensure regression tests are passing.

## Commit instruction
- Always run `cargo clippy` and `cargo test` before committing. Ensure code changes are properly formatted with `cargo fmt`.
