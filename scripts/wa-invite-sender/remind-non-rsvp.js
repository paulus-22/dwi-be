#!/usr/bin/env node
/**
 * remind-non-rsvp.js — send a WhatsApp reminder to guests who haven't RSVP'd.
 *
 * Picks every guest of an event with NO RSVP yet (attending IS NULL), writes
 * them in send.js's CSV format, then hands the list to the existing WhatsApp
 * sender. Re-running never double-reminds: send.js keeps its own sent-log
 * (default here `reminder-log.csv`), and guests who already confirmed are
 * excluded at the source.
 *
 * Usage:
 *   node remind-non-rsvp.js --slug tari-saefulloh [--dry-run] [--limit N]
 *     [--no-send] [--template reminder-template.txt]
 *     [--log reminder-log.csv] [--days-before 14] [--site-url https://themoments.id]
 *
 *   --slug <slug>     Required. The event whose non-RSVP guests to remind.
 *   --days-before <n> Only remind guests whose event is within n days from now
 *                     (i.e. the event happens in <= n days). Omit = all, no filter.
 *   --dry-run         Print what would be sent; never opens WhatsApp.
 *   --no-send         Write the reminder CSV but don't hand to the sender.
 *   --limit N         Hand only the first N not-yet-reminded guests.
 *   --template <path> Message file; placeholders {{name}}, {{link}}, {{category}}.
 *                     Defaults to reminder-template.txt if it exists.
 *   --log <path>      Sent-log for reminders (default reminder-log.csv).
 *   --out <path>      Reminder CSV for the sender (default reminders-<slug>-<date>.csv).
 *   --site-url <url>  Base for links (default https://themoments.id).
 *
 * DB from $DATABASE_URL or dwi-be/.env. Requires npm package 'pg'.
 */
"use strict";

const fs = require("fs");
const path = require("path");
const { Client } = require("pg");

function parseArgs(argv) {
  const args = {
    siteUrl: "https://themoments.id",
    log: "reminder-log.csv",
    template: path.join(__dirname, "reminder-template.txt"),
  };
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i], next = () => argv[++i];
    if (a === "--slug") args.slug = next();
    else if (a === "--site-url") args.siteUrl = next();
    else if (a === "--out") args.out = next();
    else if (a === "--log") args.log = next();
    else if (a === "--template") args.template = next();
    else if (a === "--days-before") args.daysBefore = Number(next());
    else if (a === "--dry-run") args.dryRun = true;
    else if (a === "--no-send") args.noSend = true;
    else if (a === "--limit") args.limit = Number(next());
    else if (a === "--delay-min") args.delayMin = Number(next());
    else if (a === "--delay-max") args.delayMax = Number(next());
    else if (a === "--help" || a === "-h") args.help = true;
    else throw new Error(`Unknown argument: ${a}`);
  }
  return args;
}

function findDbUrl() {
  if (process.env.DATABASE_URL) return process.env.DATABASE_URL;
  for (const p of [`.env`, `../.env`, `${__dirname}/../../.env`]) {
    try {
      const m = fs.readFileSync(p, "utf8").match(/^DATABASE_URL=(.*)$/m);
      if (m) return m[1].replace(/["']/g, "").trim();
    } catch {}
  }
  throw new Error("DATABASE_URL not set and no dwi-be/.env found");
}

function forceNodePath() {
  const exe = process.execPath || "";
  const base = exe.split(/[\\/]/).pop() || "";
  return /^(bun|bunx)$/i.test(base) ? "node" : exe;
}

function escCsv(v) {
  const s = String(v ?? "");
  return /[",\n]/.test(s) ? `"${s.replace(/"/g, '""')}"` : s;
}

async function main() {
  const args = parseArgs(process.argv.slice(2));
  if (args.help) {
    console.log(`remind-non-rsvp.js — WhatsApp reminder for guests with no RSVP.\n
  node remind-non-rsvp.js --slug <event-slug> [options]

Options:
  --slug <slug>      Required. Event whose non-RSVP guests to remind.
  --days-before <n>  Only guests whose event is within n days (default: no filter).
  --dry-run          Print candidates without opening WhatsApp.
  --no-send          Write the CSV only, don't hand to send.js.
  --limit N          Remind only the first N not-yet-reminded guests.
  --template <path>  Message file ({{name}}, {{link}}, {{category}}).
  --log <path>       Reminder sent-log (default reminder-log.csv).
  --out <path>       Reminder CSV for the sender.
  --site-url <url>   Base for links (default https://themoments.id).

DB from $DATABASE_URL or dwi-be/.env. Requires npm package 'pg'.`);
    process.exit(0);
  }
  if (!args.slug) throw new Error("--slug is required");

  const client = new Client({ connectionString: findDbUrl() });
  await client.connect();

  const ev = (
    await client.query(
      `SELECT id, slug, "headlineDate", "timezone"
         FROM event WHERE slug=$1 LIMIT 1`,
      [args.slug],
    )
  ).rows[0];
  if (!ev) throw new Error(`Event not found: ${args.slug}`);

  // Guests with a phone who have not RSVP'd yet.
  let sql = `
    SELECT g.token AS code,
           COALESCE(r."respondentName", g."householdName") AS name,
           g.category AS cat, g.phone, e.slug
      FROM guest g
      JOIN event e ON e.id = g."eventId"
      LEFT JOIN rsvp r ON r."guestId" = g.id
     WHERE e.slug = $1
       AND g.phone IS NOT NULL AND btrim(g.phone) <> ''
       AND r.attending IS NULL`;
  const params = [args.slug];

  if (args.daysBefore) {
    // Only guests whose event happens within `daysBefore` days from now.
    sql += `
       AND e."headlineDate" AT TIME ZONE e."timezone" <= now() + make_interval(days => $2)`;
    params.push(args.daysBefore);
  }

  sql += `
     ORDER BY g."sequence"`;

  const { rows } = await client.query(sql, params);
  await client.end();

  console.log(
    `Event ${ev.slug}: ${rows.length} guest(s) with no RSVP${args.daysBefore ? `, event within ${args.daysBefore}d` : ""}.`,
  );
  if (rows.length === 0) {
    console.log("Nothing to remind — everyone has responded.");
    return;
  }

  args.out = args.out || `reminders-${ev.slug}-${new Date().toISOString().slice(0, 10)}.csv`;
  const lines = rows.map(
    (g) =>
      `${escCsv(g.name)},${escCsv(g.phone)},${escCsv(g.cat ?? "umum")},${args.siteUrl}/e/${g.slug}?to=${g.code}`,
  );
  fs.writeFileSync(args.out, lines.join("\n") + "\n");
  console.log(`Wrote ${lines.length} reminder rows to ${args.out}`);

  if (args.noSend || lines.length === 0) {
    console.log(args.noSend ? "Skipping WhatsApp (--no-send)." : "Nothing to send.");
    return;
  }

  // Hand off to the existing sender under node (whatsapp-web.js is unreliable
  // under Bun), with ITS OWN sent-log so reminders don't collide with the
  // original invites.
  const { spawnSync } = require("child_process");
  const sendArgs = ["send.js", "--csv", args.out, "--log", args.log];
  if (args.template && !["", "none"].includes(args.template) && fs.existsSync(args.template)) {
    sendArgs.push("--template", args.template);
  }
  if (args.dryRun) sendArgs.push("--dry-run");
  if (args.limit) sendArgs.push("--limit", String(args.limit));
  if (args.delayMin) sendArgs.push("--delay-min", String(args.delayMin));
  if (args.delayMax) sendArgs.push("--delay-max", String(args.delayMax));
  const r = spawnSync(forceNodePath(), sendArgs, { cwd: __dirname, stdio: "inherit" });
  process.exit(r.status === null ? 1 : r.status);
}

main().catch((e) => {
  console.error(e.message);
  process.exit(1);
});
