#!/usr/bin/env node
/**
 * import-and-send.js — import a guest CSV into the database, then optionally
 * send the invitation links over WhatsApp (via the existing send.js).
 *
 * Flow:
 *   1. reads your CSV:  name,phone,category[,link]
 *      (headerless, like export_guests_for_wa.sql; a leftover header row is
 *      detected and skipped)
 *   2. for each row, generates the exact invite code the app uses
 *      (`{codePrefix}-{CAT}-{seq:04}-{check}`) — same checksum as the Rust
 *      backend — and inserts the guest (upsert on event+phone so re-running
 *      never duplicates; unmatched categories map to GEN/umum)
 *   3. writes the imported rows to `guests.csv` in send.js's format
 *   4. hands the list to send.js (which prints the QR on first real send)
 *
 * Usage:
 *   node import-and-send.js --csv list.csv --slug tari-saefulloh [--dry-run]
 *     [--limit N] [--no-send] [--site-url https://themoments.id]
 *
 *   --dry-run    import + print what would be sent, open nothing
 *   --no-send    import only, do NOT hand off to WhatsApp
 *   --limit N    hand only the first N imported guests to the sender
 *   --site-url   base for the links (default https://themoments.id)
 *
 * DB access comes from $DATABASE_URL or dwi-be/.env (same file the export
 * script reads). Requires the `pg` npm package (npm install pg).
 */
"use strict";

const fs = require("fs");
const path = require("path");
const { Client } = require("pg");

// ---- invite code helpers (mirror of dwi-be crates/domain/src/invite_code.rs)
const ALPHABET = "23456789ABCDEFGHJKMNPQRTUVWXYZ";

const CAT_CODE = {
  keluarga: "FAM", teman: "FRN", kolega: "WRK", vip: "VIP", vvip: "VIP", umum: "GEN",
};
// label shown in the app for each code (same as CategoryCode::label)
const CAT_LABEL = { FAM: "keluarga", FRN: "teman", WRK: "kolega", VIP: "vip", GEN: "umum" };

function categoryCode(raw) {
  const t = String(raw || "umum").trim().toLowerCase();
  if (CAT_CODE[t]) return CAT_CODE[t];
  if (/^(famili|family|saudara)$/.test(t)) return "FAM";
  if (/^(sahabat|friend|friends)$/.test(t)) return "FRN";
  if (/^(kantor|kerja|rekan|work|colleague|office)$/.test(t)) return "WRK";
  if (/^(undangan)$/.test(t)) return "VIP";
  return "GEN";
}
// position-weighted checksum (duplicate of the Rust check_character)
function checkCharacter(seqRaw, category, prefix) {
  const body = `${prefix}-${category}-${seqRaw.toString().padStart(4, "0")}`;
  let total = 0;
  for (let i = 0; i < body.length; i++) {
    const b = body.charCodeAt(i);
    const pos = ALPHABET.indexOf(String.fromCharCode(b));
    const weight = (pos < 0 ? 1 : pos + 1) * (i + 2);
    total += weight;
  }
  return ALPHABET[total % ALPHABET.length];
}
function buildInviteCode(prefix, category, seq) {
  return `${prefix}-${category}-${seq.toString().padStart(4, "0")}-${checkCharacter(seq, category, prefix)}`;
}

// ---- csv line parser (same as send.js)
function parseCsvLine(line) {
  const out = [];
  let cur = "", inQuotes = false;
  for (let i = 0; i < line.length; i++) {
    const c = line[i];
    if (inQuotes) {
      if (c === '"' && line[i + 1] === '"') { cur += '"'; i++; }
      else if (c === '"') inQuotes = false;
      else cur += c;
    } else if (c === '"') inQuotes = true;
    else if (c === ",") { out.push(cur); cur = ""; }
    else cur += c;
  }
  out.push(cur);
  return out.map((s) => s.trim());
}

function parseArgs(argv) {
  const args = { siteUrl: "https://themoments.id", out: "guests.csv" };
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i], next = () => argv[++i];
    if (a === "--csv") args.csv = next();
    else if (a === "--slug") args.slug = next();
    else if (a === "--site-url") args.siteUrl = next();
    else if (a === "--out") args.out = next();
    else if (a === "--dry-run") args.dryRun = true;
    else if (a === "--no-send") args.noSend = true;
    else if (a === "--limit") args.limit = Number(next());
    else if (a === "--help" || a === "-h") args.help = true;
    else throw new Error(`Unknown argument: ${a}`);
  }
  return args;
}

function loadRows(csvPath) {
  if (!csvPath) throw new Error("--csv is required");
  const text = fs.readFileSync(csvPath, "utf8");
  let lines = text.split(/\r?\n/).filter((l) => l.trim() !== "");
  const rows = [];
  for (const line of lines) {
    const [name, phone, category, link] = parseCsvLine(line);
    // skip a leftover header row (name/phone header test)
    if (/^name\b/i.test(name) && /^phone\b/i.test(phone)) continue;
    rows.push({ name, phone, category, link });
  }
  return rows;
}

// Resolve a *node* binary path, never the current interpreter. When run via
// `bun`, process.execPath points at Bun — the WhatsApp session must run under
// node (whatsapp-web.js + puppeteer headless is unreliable under Bun). Falls
// back to plain "node" from PATH if execPath looks like Bun.
function forceNodePath() {
  const exe = process.execPath || "";
  const base = exe.split(/[\\/]/).pop() || "";
  if (!/^(bun|bunx)$/i.test(base)) {
    return exe; // already node (or node-like)
  }
  return "node"; // else let the shell resolve the system node
}

function findDbUrl() {
  if (process.env.DATABASE_URL) return process.env.DATABASE_URL;
  // fall back to the repo's .env (same one export_guests_for_wa.sql reads)
  for (const p of [`.env`, `../.env`, `${__dirname}/../../.env`]) {
    try {
      const m = fs.readFileSync(p, "utf8").match(/^DATABASE_URL=(.*)$/m);
      if (m) return m[1].replace(/["']/g, "").trim();
    } catch {}
  }
  throw new Error("DATABASE_URL not set and no dwi-be/.env found");
}

async function main() {
  const args = parseArgs(process.argv.slice(2));
  if (args.help) {
    console.log(`import-and-send.js — import guest CSV, then send over WhatsApp.

  node import-and-send.js --csv list.csv --slug <event-slug> [options]

Options:
  --slug <slug>      Restrict import to this event. If omitted, the first event
                     in the DB is used.
  --site-url <url>   Base for invite links (default https://themoments.id).
  --dry-run          Import + print, but don't open WhatsApp and send nothing.
  --no-send          Import only; write guests.csv but don't hand to send.js.
  --limit N          Send to only the first N imported guests.
  --out <file>       Where to write the send-formatted CSV (default guests.csv).

CSV (headerless): name,phone,category[,link]   — link column is optional.
DB from $DATABASE_URL or dwi-be/.env. Requires npm package 'pg'.`);
    process.exit(0);
  }

  const rows = loadRows(args.csv);
  if (rows.length === 0) throw new Error("No guest rows found in CSV.");
  console.log(`Read ${rows.length} guests from ${args.csv}`);

  const client = new Client({ connectionString: findDbUrl() });
  await client.connect();

  // resolve event
  const ev = args.slug
    ? (await client.query(`SELECT id, slug, "codePrefix" FROM event WHERE slug=$1`, [args.slug])).rows[0]
    : (await client.query(`SELECT id, slug, "codePrefix" FROM event ORDER BY "createdAt" DESC LIMIT 1`)).rows[0];
  if (!ev) throw new Error(`Event not found: ${args.slug}`);
  console.log(`Event: ${ev.slug} (prefix ${ev.codePrefix})`);

  // compute next sequence — same global-prefix-scoped rule as the backend
  const seqRow = await client.query(
    `SELECT COALESCE(MAX(g."sequence"),0)+1 AS n FROM guest g
       JOIN event e ON e.id=g."eventId" WHERE e."codePrefix"=$1`,
    [ev.codePrefix],
  );
  let nextSeq = Number(seqRow.rows[0].n);

  const outLines = [];
  let inserted = 0, skipped = 0;
  const seen = new Set();

  for (const r of rows) {
    const cat = categoryCode(r.category);
    const phone = (r.phone || "").trim();
    if (!r.name.trim() || !phone) { skipped++; continue; }
    // skip exact duplicate phone within this import batch
    const key = phone.replace(/\D/g, "");
    if (seen.has(key)) { skipped++; continue; }
    seen.add(key);

    const seq = nextSeq++;
    const token = buildInviteCode(ev.codePrefix, cat, seq);
    const link = r.link && /^https?:\/\//.test(r.link)
      ? r.link
      : `${args.siteUrl}/e/${ev.slug}?to=${token}`;
    const label = CAT_LABEL[cat];

    // Existing guest with this phone on this event? Keep their token (import
    // is idempotent — re-running never duplicates a guest).
    const existing = await client.query(
      `SELECT id, token FROM guest WHERE "eventId"=$1 AND phone=$2 LIMIT 1`,
      [ev.id, phone],
    );
    let finalToken = token;
    if (existing.rows.length === 0) {
      await client.query(
        `INSERT INTO guest (id,"eventId","householdName",token,"plusGuestAllowance",category,phone,"invitedAt","updatedAt",sequence)
         VALUES (gen_random_uuid(),$1,$2,$3,0,$4,$5,now(),now(),$6)`,
        [ev.id, r.name.trim(), token, label, phone, seq],
      );
    } else {
      finalToken = existing.rows[0].token;
    }
    inserted++;

    outLines.push(`${r.name.trim()},${phone},${label},${args.siteUrl}/e/${ev.slug}?to=${finalToken}`);
  }
  await client.end();

  console.log(`Imported ${inserted} (${skipped} skipped: missing phone/name, duplicate, or no row).`);
  fs.writeFileSync(args.out, outLines.join("\n") + (outLines.length ? "\n" : ""));
  console.log(`Wrote ${outLines.length} rows for sending to ${args.out}`);

  if (args.noSend || outLines.length === 0) {
    console.log(args.noSend ? "Skipping WhatsApp (--no-send)." : "Nothing to send.");
    return;
  }

  // hand off to the existing sender.
  // IMPORTANT: spawn the WhatsApp session under node, never the current
  // interpreter (e.g. Bun). whatsapp-web.js / puppeteer's headless injection
  // is unreliable under Bun and times out (30s) launching WhatsApp Web.
  const nodeBin = forceNodePath();
  const sendArgs = ["send.js", "--csv", args.out];
  if (args.dryRun) sendArgs.push("--dry-run");
  if (args.limit) sendArgs.push("--limit", String(args.limit));
  const { spawnSync } = require("child_process");
  const r = spawnSync(nodeBin, sendArgs, { cwd: __dirname, stdio: "inherit" });
  process.exit(r.status === null ? 1 : r.status);
}

main().catch((e) => { console.error(e.message); process.exit(1); });
