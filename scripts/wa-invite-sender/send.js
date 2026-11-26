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
    else if (a === "--batch-size") args.batchSize = Number(next());
    else if (a === "--pause-between") args.pauseBetween = Number(next());
    else if (a === "--log") args.log = next();
    else if (a === "--limit") args.limit = Number(next());
    else if (a === "--template") args.templatePath = next();
    else if (a === "--context") args.contextPath = next();
    else if (a === "--session") args.session = next();
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
  --batch-size <n>       Pause after every n successful sends (default: no batching).
  --pause-between <sec>  How long to pause between batches (e.g. 90). Use with
                         --batch-size so a large blast looks human and dodges
                         WhatsApp's spam throttle.
  --log <path>          Sent-log CSV for resume/skip (default sent-log.csv).
  --limit <n>            Only send to the first N not-yet-sent guests.
  --session <name>       Which WhatsApp account to send from. Each name keeps
                         its own auth dir (.wwebjs_auth-<name>), so you can
                         send from two numbers (e.g. --session tari and
                         --session iful). Default: .wwebjs_auth.
  --template <path>      Message template file. Placeholders: {{name}},
                        {{link}}, {{category}}, plus from --context:
                        {{date}}, {{time}}, {{venue}}, {{maps}}.
  --context <json>       File with {date,time,venue,maps} to fill the event
                         placeholders above.
`;

const DEFAULT_TEMPLATE = `Yth.
Bapak/Ibu/Saudara/i
{{name}}
di Tempat

Bismillahirrahmanirrahim,
Assalamu’alaikum Warahmatullahi Wabarakatuh.

Tanpa mengurangi rasa hormat, perkenankan kami mengundang Bapak/Ibu/Saudara/i untuk hadir dan memberikan doa restu pada hari bahagia pernikahan kami.

Informasi lengkap mengenai detail acara serta konfirmasi kehadiran dapat diakses melalui tautan undangan digital berikut:

{{link}}

Sampai bertemu di hari bahagia kami. 🤍

Terima kasih
Wassalamu’alaikum Warahmatullahi Wabarakatuh

Kami yang berbahagia,
Tari & Iful`;

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

/**
 * Normalizes an Indonesian-style number to WhatsApp's `@c.us` JID.
 *  - "+62 …" / "+60 …" / "+65 …": explicit international format is kept as-is
 *    (leading zero in local format is dropped per country-code rules)
 *  - "0812…" (starts with 0): Indonesian local → 62812…
 *  - bare digits without country code: assumed Indonesian → 62812…
 */
function toWhatsAppId(rawPhone) {
  let digits = rawPhone.replace(/[^\d+]/g, "");
  if (digits.startsWith("+")) {
    // explicit international format: keep the full country code + national
    // number, but strip a leading "0" that some people write after "+62"
    digits = digits.slice(1);
    if (digits.startsWith("62") && digits.length > 10) digits = digits.replace(/^620/, "620");
    if (digits.startsWith("0")) digits = digits.slice(1);
    return `${digits}@c.us`;
  }
  if (digits.startsWith("0")) {
    digits = "62" + digits.slice(1); // Indonesian local format
  } else if (!digits.startsWith("62") && (digits.length < 10 || digits.startsWith("8"))) {
    // short or Indonesian (8…) number without country code
    digits = "62" + digits;
  }
  return `${digits}@c.us`;
}

function renderTemplate(tpl, guest, ctx = {}) {
  return tpl
    .replaceAll("{{name}}", guest.name)
    .replaceAll("{{link}}", guest.link)
    .replaceAll("{{category}}", guest.category)
    .replaceAll("{{date}}", ctx.date || "")
    .replaceAll("{{time}}", ctx.time || "")
    .replaceAll("{{venue}}", ctx.venue || "")
    .replaceAll("{{maps}}", ctx.maps || "")
    .trim();
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
  let ctx = {};
  if (args.contextPath) {
    try {
      ctx = JSON.parse(fs.readFileSync(args.contextPath, "utf8"));
    } catch {
      console.error(`Could not read --context ${args.contextPath}; continuing with empty placeholders.`);
    }
  }
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
      console.log(renderTemplate(template, g, ctx));
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

  const authDir = path.join(
    __dirname,
    args.session ? `.wwebjs_auth-${args.session.replace(/[^a-z0-9_-]/gi, "")}` : ".wwebjs_auth",
  );

  const client = new Client({
    authStrategy: new LocalAuth({ dataPath: authDir }),
    // authTimeoutMs is the gate `inject()` waits on (window.Debug.VERSION).
    // Default is 30s; first launch / a freshly-scanned QR routinely takes
    // longer, so raise it or the session dies with "Waiting failed:
    // 30000ms exceeded". protocolTimeout below is a *different* knob.
    authTimeoutMs: 180000,
    // launchTimeout + protocolTimeout: first launch often takes >30s to load
    // the (heavy) WhatsApp Web page; without these the session crashes with
    // "Waiting failed: 30000ms exceeded" at inject().
    // If raising authTimeoutMs still isn't enough (WA blocks headless), run
    // with a real window: WA_HEADLESS=0 node send.js …
    puppeteer: {
      headless: process.env.WA_HEADLESS !== "0",
      protocolTimeout: 180000,
      timeout: 120000,
      args: [
        "--no-sandbox",
        "--disable-setuid-sandbox",
        "--disable-dev-shm-usage",
        "--user-agent=Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36",
      ],
    },
    takeoverOnConflict: true,
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
      await client.sendMessage(jid, renderTemplate(template, g, ctx));
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
      if (args.batchSize && okCount > 0 && okCount % args.batchSize === 0 && args.pauseBetween) {
        console.log(`\nBatch done (${okCount} sent). Pausing ${args.pauseBetween}s before the next batch…`);
        await sleep(args.pauseBetween * 1000);
        process.stdout.write("\n");
      } else {
        const delay = args.delayMin + Math.random() * (args.delayMax - args.delayMin);
        await sleep(delay * 1000);
      }
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
