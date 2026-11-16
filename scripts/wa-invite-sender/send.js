#!/usr/bin/env node
/**
 * Send wedding invitations over WhatsApp Web, logged in as the couple's own
 * number (QR scan on first run, session persists after that — see README).
 *
 * Usage:
 *   node send.js --csv guests.csv [--dry-run] [--delay-min 15] [--delay-max 35]
 *                [--log sent-log.csv] [--limit N] [--template template.txt]
 *
 * CSV columns (no header), one row per guest, from export_guests_for_wa.sql:
 *   name,phone,category,link
 *
 * This is semi-automated bulk messaging from a personal WhatsApp account —
 * WhatsApp can rate-limit or temporarily restrict accounts that send too
 * many messages too fast to numbers that haven't messaged them first. The
 * defaults here are deliberately conservative (15–35s between sends, a
 * confirmation prompt before any real send, and a resumable sent-log so a
 * run can be safely stopped and restarted). Don't remove the delay to "go
 * faster" without accepting that risk.
 */
"use strict";

const fs = require("fs");
const path = require("path");
const readline = require("readline");

function parseArgs(argv) {
  const args = { delayMin: 15, delayMax: 35, log: "sent-log.csv" };
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    const next = () => argv[++i];
    if (a === "--csv") args.csv = next();
    else if (a === "--dry-run") args.dryRun = true;
    else if (a === "--delay-min") args.delayMin = Number(next());
    else if (a === "--delay-max") args.delayMax = Number(next());
    else if (a === "--log") args.log = next();
    else if (a === "--limit") args.limit = Number(next());
    else if (a === "--template") args.templatePath = next();
    else if (a === "--help" || a === "-h") args.help = true;
    else throw new Error(`Unknown argument: ${a}`);
  }
  return args;
}

const HELP = `
wa-invite-sender — send invitation links over WhatsApp Web

  node send.js --csv guests.csv [options]

Options:
  --csv <path>          Required. CSV: name,phone,category,link (no header).
  --dry-run             Print what would be sent; never opens WhatsApp Web.
  --delay-min <sec>      Minimum delay between sends (default 15).
  --delay-max <sec>      Maximum delay between sends (default 35).
  --log <path>          Sent-log CSV for resume/skip (default sent-log.csv).
  --limit <n>            Only send to the first N not-yet-sent guests.
  --template <path>      Message template file. Placeholders: {{name}},
                          {{link}}, {{category}}. Default is built into
                          this script — see DEFAULT_TEMPLATE below.
`;

const DEFAULT_TEMPLATE = `Halo {{name}},

Dengan penuh syukur dan sukacita, kami mengundang Anda untuk hadir di acara pernikahan kami.

Silakan buka undangan digital kami di tautan berikut untuk info lengkap dan konfirmasi kehadiran:
{{link}}

Merupakan kebahagiaan bagi kami apabila Anda dapat hadir dan memberikan restu.

Terima kasih 🙏`;

/** Minimal CSV line parser: handles quoted fields with embedded commas. */
function parseCsvLine(line) {
  const out = [];
  let cur = "";
  let inQuotes = false;
  for (let i = 0; i < line.length; i++) {
    const c = line[i];
    if (inQuotes) {
      if (c === '"' && line[i + 1] === '"') {
        cur += '"';
        i++;
      } else if (c === '"') {
        inQuotes = false;
      } else {
        cur += c;
      }
    } else if (c === '"') {
      inQuotes = true;
    } else if (c === ",") {
      out.push(cur);
      cur = "";
    } else {
      cur += c;
    }
  }
  out.push(cur);
  return out.map((s) => s.trim());
}

function loadGuests(csvPath) {
  const text = fs.readFileSync(csvPath, "utf8");
  const lines = text.split(/\r?\n/).filter((l) => l.trim() !== "");
  return lines.map((line, i) => {
    const [name, phone, category, link] = parseCsvLine(line);
    if (!name || !phone || !link) {
      throw new Error(`guests CSV line ${i + 1}: missing name/phone/link — got: ${line}`);
    }
    return { name, phone, category: category || "umum", link };
  });
}

/** Normalizes an Indonesian-style number to WhatsApp's `@c.us` JID. */
function toWhatsAppId(rawPhone) {
  let digits = rawPhone.replace(/[^\d+]/g, "");
  if (digits.startsWith("+")) digits = digits.slice(1);
  if (digits.startsWith("0")) digits = "62" + digits.slice(1);
  if (!digits.startsWith("62") && digits.length <= 11) digits = "62" + digits;
  return `${digits}@c.us`;
}

function renderTemplate(tpl, guest) {
  return tpl
    .replaceAll("{{name}}", guest.name)
    .replaceAll("{{link}}", guest.link)
    .replaceAll("{{category}}", guest.category);
}

function loadSentLog(logPath) {
  const sent = new Set();
  if (!fs.existsSync(logPath)) return sent;
  const text = fs.readFileSync(logPath, "utf8");
  for (const line of text.split(/\r?\n/)) {
    const [phone, status] = line.split(",");
    if (phone && status === "sent") sent.add(phone.trim());
  }
  return sent;
}

function appendLog(logPath, phone, status, detail) {
  const row = [phone, status, new Date().toISOString(), (detail || "").replaceAll(",", ";")].join(",") + "\n";
  fs.appendFileSync(logPath, row);
}

function sleep(ms) {
  return new Promise((r) => setTimeout(r, ms));
}

function confirm(question) {
  return new Promise((resolve) => {
    const rl = readline.createInterface({ input: process.stdin, output: process.stdout });
    rl.question(question, (answer) => {
      rl.close();
      resolve(answer.trim().toLowerCase() === "yes");
    });
  });
}

async function main() {
  const args = parseArgs(process.argv.slice(2));
  if (args.help || !args.csv) {
    console.log(HELP);
    process.exit(args.help ? 0 : 1);
  }

  const template = args.templatePath ? fs.readFileSync(args.templatePath, "utf8") : DEFAULT_TEMPLATE;
  const allGuests = loadGuests(args.csv);
  const sent = loadSentLog(args.log);
  let pending = allGuests.filter((g) => !sent.has(g.phone));
  if (args.limit) pending = pending.slice(0, args.limit);

  console.log(`Guests in CSV: ${allGuests.length}. Already sent (per ${args.log}): ${sent.size}. Pending this run: ${pending.length}.`);
  if (pending.length === 0) {
    console.log("Nothing to send.");
    return;
  }

  if (args.dryRun) {
    console.log("\n--- DRY RUN: no WhatsApp session opened, nothing sent ---\n");
    for (const g of pending) {
      console.log(`→ ${g.name} (${g.phone}, ${g.category}) [${toWhatsAppId(g.phone)}]`);
      console.log(renderTemplate(template, g));
      console.log("---");
    }
    return;
  }

  const estSeconds = pending.length * ((args.delayMin + args.delayMax) / 2);
  console.log(`\nAbout to send ${pending.length} real WhatsApp messages.`);
  console.log(`Estimated time: ~${Math.round(estSeconds / 60)} minutes (${args.delayMin}-${args.delayMax}s between sends).`);
  const ok = await confirm('Type "yes" to proceed, anything else to abort: ');
  if (!ok) {
    console.log("Aborted — nothing was sent.");
    return;
  }

  // Required lazily: only needed for a real send, so --dry-run never needs
  // whatsapp-web.js/puppeteer installed or a Chromium download.
  const { Client, LocalAuth } = require("whatsapp-web.js");
  const qrcode = require("qrcode-terminal");

  const client = new Client({
    authStrategy: new LocalAuth({ dataPath: path.join(__dirname, ".wwebjs_auth") }),
    puppeteer: { headless: true },
  });

  client.on("qr", (qr) => {
    console.log("\nScan this QR with WhatsApp on the groom/bride's phone (Linked Devices → Link a Device):\n");
    qrcode.generate(qr, { small: true });
  });

  client.on("authenticated", () => console.log("WhatsApp session authenticated."));
  client.on("auth_failure", (m) => {
    console.error("Auth failure:", m);
    process.exit(1);
  });

  const ready = new Promise((resolve) => client.on("ready", resolve));
  console.log("Starting WhatsApp Web session...");
  await client.initialize();
  await ready;
  console.log("WhatsApp Web is ready.\n");

  let okCount = 0;
  let failCount = 0;
  for (const g of pending) {
    const jid = toWhatsAppId(g.phone);
    try {
      const registered = await client.isRegisteredUser(jid);
      if (!registered) {
        console.log(`✗ ${g.name} (${g.phone}): not a WhatsApp number, skipping.`);
        appendLog(args.log, g.phone, "skipped_not_whatsapp", "");
        failCount++;
        continue;
      }
      await client.sendMessage(jid, renderTemplate(template, g));
      console.log(`✓ ${g.name} (${g.phone}) sent.`);
      appendLog(args.log, g.phone, "sent", "");
      okCount++;
    } catch (e) {
      console.error(`✗ ${g.name} (${g.phone}) failed: ${e.message}`);
      appendLog(args.log, g.phone, "failed", e.message);
      failCount++;
    }
    const isLast = g === pending[pending.length - 1];
    if (!isLast) {
      const delay = args.delayMin + Math.random() * (args.delayMax - args.delayMin);
      await sleep(delay * 1000);
    }
  }

  console.log(`\nDone. Sent: ${okCount}. Failed/skipped: ${failCount}. See ${args.log} for the full log.`);
  await client.destroy();
  process.exit(0);
}

main().catch((e) => {
  console.error(e);
  process.exit(1);
});
