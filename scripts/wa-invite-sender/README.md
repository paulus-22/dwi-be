# WhatsApp invitation sender

Sends each guest their personal invitation link over WhatsApp, logged in as
the groom's or bride's own number (via WhatsApp Web, no business API, no
token to pay for). This is **not** the `whatsapp.rs` Fonnte integration in
`crates/api` (that's the *automatic RSVP confirmation* message sent by the
backend when a guest RSVPs — separate thing, separate account/number). This
script is for the manual, one-time (or occasional) *initial invitation*
blast, run by hand from your own machine.

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
  -v site_url="'https://preview-tm.luspau.my.id'" \
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
- `guests.csv`, `sent-log.csv`, `.wwebjs_auth/` — all gitignored; these are
  real guest data / a live login session, never commit them.
