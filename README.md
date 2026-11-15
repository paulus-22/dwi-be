# dwi-api

Rust backend for the DWI wedding invitation platform: admin API, door-scanner
sync, live attendance, and photo uploads.

- **axum 0.8** HTTP, **sqlx 0.9** + Postgres (owns the schema), **Redis**
  (optional) for caching, rate limits and multi-instance live updates.
- Photos go straight from the device to **Cloudflare R2** with presigned URLs.
- `crates/domain` holds the invite-code and QR-signing rules, ported byte for
  byte from the original TypeScript app and proven by golden vectors
  (`crates/domain/tests/golden.json`, generated from the TS code). If those
  tests ever fail, printed cards stop scanning: fix the code, not the vectors.

```
crates/
  domain/   pure rules, no IO (invite codes, QR payload HMAC)
  api/      the server: routes, auth, sync, live, storage
migrations/
  0001_baseline.sql   the old app's Prisma schema, verbatim
  0002_scanner.sql    devices, pairing codes, scan log, photos
```

## Run locally

Needs Rust (stable), PostgreSQL and, optionally, Redis.

```bash
brew install rustup postgresql@18 redis && rustup default stable
brew services start postgresql@18 && brew services start redis
createdb dwi_api_dev

cp .env.example .env           # then set JWT_SECRET and INVITE_SIGNING_SECRET
cargo run -- seed-demo         # Alex & Ajeng event + admin@demo.local / DEMO_ADMIN_PASSWORD
cargo run                      # migrates, then serves on :8080
bash scripts/smoke.sh          # login → pair → sync → live stream, against the running server
```

`INVITE_SIGNING_SECRET` **must equal the old app's value** in any shared
environment, or QR codes on printed cards will not verify.

## Test

```bash
createdb dwi_api_test
DATABASE_URL=postgres://localhost/dwi_api_test TEST_REDIS_URL=redis://127.0.0.1:6379 cargo test
cargo clippy --all-targets -- -D warnings && cargo fmt --check
```

Each integration test gets its own fresh database (`#[sqlx::test]`). Without
`TEST_REDIS_URL` the three Redis tests skip themselves.

## API

All JSON. Errors are `{ "error": { "code", "message" } }`. `code` is stable for
clients to branch on, and `message` is Indonesian so it can be shown to people.

| Method | Path | Who | What |
| --- | --- | --- | --- |
| POST | `/v1/auth/login` | — | `{email, password}` → `{token, expiresAt, admin}` (rate-limited with Redis) |
| POST | `/v1/auth/password` | admin | `{current, next}` replaces a temporary password |
| GET | `/v1/me` | admin | the signed-in admin |
| GET | `/v1/events` | admin | events you can see, with your role |
| GET | `/v1/events/:id` | viewer+ | one event |
| GET | `/v1/events/:id/guests` | viewer+ | guests with RSVP and first check-in |
| GET | `/v1/events/:id/attendance` | viewer+ | totals, per-door counts, latest arrivals |
| GET/POST | `/v1/events/:id/pairing-codes` | organizer+ | list / create (`{label?, validHours?}`) |
| DELETE | `/v1/pairing-codes/:id` | organizer+ | revoke a code |
| GET | `/v1/events/:id/devices` | viewer+ | paired door devices |
| DELETE | `/v1/devices/:id` | organizer+ | revoke a device (its token dies at once) |
| POST | `/v1/devices/pair` | — | `{code, deviceName}` → `{event, deviceToken}` |
| GET | `/v1/events/:id/snapshot` | device | guest list + all check-ins + `cursor` |
| POST | `/v1/sync` | device | push check-ins/records, pull others' check-ins |
| POST | `/v1/uploads/presign` | device | `{photoId, contentType}` → presigned R2 PUT |
| POST | `/v1/photos` | device | register an uploaded photo |
| GET | `/v1/events/:id/photos?after=` | admin or device | photo wall, paginated by `seq` |
| GET | `/v1/events/:id/live` | admin or device | Server-Sent Events: `ready`, `scan`, `photo`, `resync` |
| GET | `/healthz` | — | db / redis / storage status |

Admins send `Authorization: Bearer <jwt>`. Devices send `Bearer dwd_…`.
`EventSource` cannot set headers, so `/live` also accepts `?access_token=`.

Roles are the same as the old app: the super admin sees everything; everyone
else reaches an event through `event_membership` as `VIEWER < ORGANIZER < OWNER`.
An unknown event and an inaccessible one both return 404.

### Scanner sync protocol

Built for bad venue wifi:

1. **Pair.** An organiser creates a pairing code (`XXXX-XXXX`, valid 24 h by
   default, one code for every door). The device exchanges it for a token
   scoped to that one event. Only the token's SHA-256 is stored.
2. **Snapshot.** The device downloads the guest list (cached 15 s in Redis) and
   every check-in so far, plus a `cursor`.
3. **Sync loop** (every few seconds, and when back online): `POST /v1/sync` with
   `{deviceNow, cursor, checkIns[], records[]}`.
   - Every row has an id generated on the device, so a retried batch never
     duplicates. `acked` lists every id the server now holds; `rejected` lists
     invalid rows, which shouldn't be retried.
   - `deviceNow` measures clock skew; `occurredAt` is corrected by it.
   - The response carries `remoteCheckIns` from *other* doors after `cursor`,
     plus the new `cursor` (and `hasMore`).
4. **Duplicates.** Two offline doors may admit the same guest. Both scans are
   kept; attendance uses each guest's *earliest* scan, so a late-syncing device
   with an earlier scan still wins. `repeatScans` shows how many extras there were.

Check-ins are also mirrored into the old app's `check_in` table, so its admin
keeps working during the transition.

## Database and the old app

The old Next.js app (Prisma) and this API share one Postgres during the
transition:

- `0001_baseline` is the Prisma schema verbatim. On a database Prisma already
  built, `dwi-api migrate` **records** it as applied (after checking the latest
  Prisma migration is present) instead of running it, and then applies `0002`.
  This is tested against a real copy of the schema.
- The new tables are additive. The old app ignores them.
- Prisma-era tables keep their camelCase columns. New tables are snake_case.
- From cutover on, schema changes go here as new sqlx migrations. Stop running
  `prisma migrate` against shared databases.

## Deploy

Planned: **Railway** (Singapore) for this service plus Postgres; **Upstash** or
Railway Redis; **Cloudflare R2** for photos; the frontend on Cloudflare.

1. Create a Railway service from this repo. It builds the `Dockerfile` and
   health-checks `/healthz` (`railway.json`).
2. Set the variables from `.env.example`. The production values must be set to:
   - `DATABASE_URL`: the shared database;
   - `INVITE_SIGNING_SECRET`: the old app's secret;
   - `JWT_SECRET`: `openssl rand -base64 48`;
   - `CORS_ORIGINS`: the frontend and scanner origins.
3. R2: create a bucket and an API token (Object Read & Write), then set the
   `S3_*` variables. Add a CORS rule on the bucket that allows `PUT` from the
   scanner origin with the `content-type` header.
4. Migrations run on every start. Running `dwi-api migrate` once by hand first
   is a cheap way to check that adoption works against production.

> The Dockerfile has not been built locally (no Docker on the dev machine).
> CI (`.github/workflows/ci.yml`) runs fmt, clippy and the full test suite
> with Postgres 18 and Redis 8.
