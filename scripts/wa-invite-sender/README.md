# WhatsApp invitation sender

## Flow tools

Two scripts here, used in sequence for "I have the list → guests in the DB → send":

1. **`import-and-send.js`** — feed it any CSV (`name,phone,category`, headerless) and it
   inserts guests into the DB (generating app-valid invite codes
   `{prefix}-{CAT}-{seq:04}-{check}` with the exact checksum the backend uses),
   writes `guests.csv`, then hands off to `send.js`.
   ```bash
   node import-and-send.js --csv list.csv --slug tari-saefulloh --dry-run  # import + preview
   node import-and-send.js --csv list.csv --slug tari-saefulloh --limit 10 # import + send first 10
   ```
   Idempotent: re-running never duplicates a guest (matched on event+phone).
   Needs `npm install pg`.

2. **`send.js`** — the WhatsApp Web sender (see below). You usually reach it via
   `import-and-send.js`, or pass a `guests.csv` directly:
   ```bash
   node send.js --csv guests.csv --dry-run
   node send.js --csv guests.csv --limit 50
   ```

3. **`remind-non-rsvp.js`** — new script: WhatsApp **reminder** to every guest who
   has **not RSVP'd yet** on a given event. Pulls them straight from the DB,
   writes a `reminders-<slug>-<date>.csv`, and hands off to `send.js` with its
   **own** sent-log (`reminder-log.csv`) so reminders never collide with the
   original invites and re-runs skip already-reminded numbers.
   ```bash
   node remind-non-rsvp.js --slug tari-saefulloh --dry-run          # preview who'd be reminded
   node remind-non-rsvp.js --slug tari-saefulloh --limit 10         # send to first 10
   node remind-non-rsvp.js --slug tari-saefulloh --days-before 7    # only if event is within 7 days
   ```
   The default message lives in `reminder-template.txt` (placeholders
   `{{name}}`, `{{link}}`, `{{category}}`) — edit it, or pass `--template`.
   Needs `npm install pg`.

## Manual export → send (no new guests; bring an existing list up to date)

Sends each guest their personal invitation link over WhatsApp, logged in as
the groom's or bride's own number (via WhatsApp Web, no business API, no
token to pay for). This is a standalone local script, separate from the
backend — the backend no longer sends any WhatsApp messages (the old Fonnte
provider integration was removed). When a guest RSVPs or registers, the QR
link is shown in the web UI itself; this script handles the *initial
invitation* blast, run by hand from your own machine.

## How it works

1. **Export the guest list** from the production/test database to a CSV
   (name, phone, category, personal invite link) with
   `export_guests_for_wa.sql`.
2. **Run `send.js`**, which:
   - On the very first run, prints a QR code in the terminal — scan it with
     the WhatsApp you want to send from (**Settings → Linked Devices → Link
     a Device**). The session is then saved in `.wwebjs_auth/` and reused on
     later runs — you only scan once (until you log out from the phone).
   - Loops through the guest list, skipping anyone already in `sent-log.csv`
     (safe to stop with Ctrl+C and re-run — it resumes, never double-sends).
   - Sends one message every 15–35s (randomized) to look less like a bot.
   - Asks you to type `yes` before it sends anything for real.

## Setup (one time)

```bash
cd scripts/wa-invite-sender
npm install     # pulls in whatsapp-web.js + puppeteer (downloads a Chromium)
```

## 1. Export the guest list

```bash
cd ../..   # back to dwi-be root
psql "$DATABASE_URL" \
  -v slug="'tari-saefulloh'" \
  -v site_url="'https://themoments.id'" \
  -A -F',' -t \
  -f scripts/wa-invite-sender/export_guests_for_wa.sql \
  -o scripts/wa-invite-sender/guests.csv
```

Swap `slug` for whichever event you mean (use the TEST database/a demo slug
first if you want to dry-run the whole flow before touching real guests).
Open `guests.csv` and sanity-check it before sending anything — this is the
exact list that's about to get a real WhatsApp message.

## 2. Preview the messages (no WhatsApp session opened, nothing sent)

```bash
cd scripts/wa-invite-sender
node send.js --csv guests.csv --dry-run
```

Read through the printed messages. If the wording needs to change, copy
`send.js`'s `DEFAULT_TEMPLATE` into a text file, edit it, and pass
`--template your-template.txt` (placeholders: `{{name}}`, `{{link}}`,
`{{category}}`).

## 3. Send for real

```bash
node send.js --csv guests.csv
```

- First run: scan the QR code shown in the terminal with the sending
  WhatsApp account's phone.
- It will show you the guest count and ask you to type `yes` before sending
  anything.
- If it's interrupted (Ctrl+C, network issue, laptop sleeps), just re-run
  the same command — guests already logged as `sent` in `sent-log.csv` are
  skipped automatically.
- Want to test on a handful of people first? Add `--limit 3`.

## Why the delay, and why this isn't instant

WhatsApp can flag or temporarily restrict an account that sends many
messages very fast to numbers that have never messaged it. The 15–35s
randomized delay between sends and the confirmation prompt are there on
purpose — don't strip them out to go faster, especially for a guest list of
any real size (hundreds of guests at 15-35s apart is already a few hours;
plan to run it in the background, or in batches across a day or two with
`--limit`).

## Files

- `export_guests_for_wa.sql` — the DB query, parameterized by event slug.
- `send.js` — the sender. `--help` for all options.
- `import-and-send.js` — CSV → DB import + hand-off to `send.js`.
- `remind-non-rsvp.js` — reminder to non-RSVP'd guests + hand-off to `send.js`.
- `reminder-template.txt` — default reminder message.
- `guests.csv`, `sent-log.csv`, `reminder-log.csv`, `.wwebjs_auth/` — all
  gitignored; these are real guest data / a live login session, never commit them.
