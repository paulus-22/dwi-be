# Git workflow

- Never `git commit` or `git push` unless the user explicitly asks for it.
- After every piece of development work, end with a ready-to-use commit title
  in this repo's style (`feat: …`, `fix: …`, `docs: …`) so the user can commit
  it themselves.

# Project

- Rust workspace: `crates/domain` (pure rules, no IO) and `crates/api` (axum server).
- `crates/domain/tests/golden.json` comes from the original TypeScript. Never
  edit the vectors to make a test pass; printed QR cards depend on them.
- Schema changes are new files in `migrations/` (sqlx). Never edit an applied
  migration. Prisma-era tables keep camelCase quoted columns; new tables use snake_case.
- Prisma generated ids and `updatedAt` in the app: when inserting into those
  tables, set `"id"` (gen_random_uuid()) and `"updatedAt"` (now()) explicitly.
- Error messages returned to clients are Indonesian; `code` values are stable English snake_case.
- Verify: `cargo fmt --check && cargo clippy --all-targets -- -D warnings &&
  DATABASE_URL=postgres://localhost/dwi_api_test TEST_REDIS_URL=redis://127.0.0.1:6379 cargo test`
