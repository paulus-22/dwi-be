#!/usr/bin/env node
/**
 * create-admin.js — add a dashboard (admin_user) account directly in the DB.
 *
 * There is NO self-signup for admins: accounts live in the `admin_user` table
 * with a bcrypt-hashed password. This script creates (or updates) one.
 *
 * Access model (see crates/api/src/routes/auth.rs + events.rs):
 *   - `isSuperAdmin` → the account sees EVERY event.
 *   - Regular admin → only sees events it has an `event_membership` row for.
 *     Pass `--event <slug>` once per event to grant access (role OWNER by
 *     default, or ORGANIZER/VIEWER via --role).
 *
 * Usage (run from dwi-be, or anywhere with dwi-be/.env reachable):
 *   node scripts/admin/create-admin.js \
 *     --email tim@themoments.id --name "Tim" --password 'S3cret!' \
 *     [--super] [--event tari-saefulloh] [--role OWNER|ORGANIZER|VIEWER]
 *
 * DB comes from $DATABASE_URL or the dwi-be/.env file.
 * Requires:   npm install   (in scripts/admin/)
 * Then:       node create-admin.js ...
 */
"use strict";

const fs = require("fs");
const path = require("path");
const bcrypt = require("bcryptjs");
const { Client } = require("pg");

const COST = 11; // matches BCRYPT_COST in the Rust API

function parseArgs(argv) {
  const args = { role: "OWNER" };
  for (let i = 0; i < argv.length; i += 1) {
    const a = argv[i], next = () => argv[++i];
    if (a === "--email") args.email = next();
    else if (a === "--name") args.name = next();
    else if (a === "--password") args.password = next();
    else if (a === "--super") args.super = true;
    else if (a === "--event") (args.events ||= []).push(next());
    else if (a === "--role") args.role = next().toUpperCase();
    else if (a === "--help" || a === "-h") args.help = true;
    else throw new Error(`Unknown argument: ${a}`);
  }
  return args;
}

function dbUrl() {
  if (process.env.DATABASE_URL) return process.env.DATABASE_URL;
  for (const p of ["../.env", path.join(__dirname, "../../.env"), "../../.env"]) {
    try {
      const m = fs.readFileSync(p, "utf8").match(/^DATABASE_URL=(.*)$/m);
      if (m) return m[1].replace(/["']/g, "").trim();
    } catch {}
  }
  throw new Error("DATABASE_URL not set and no dwi-be/.env found");
}

const ROLES = new Set(["OWNER", "ORGANIZER", "VIEWER"]);

async function main() {
  const args = parseArgs(process.argv.slice(2));
  if (args.help || !args.email || !args.password) {
    console.log(`create-admin.js — add/update a dashboard account in the DB.

Usage:
  node create-admin.js --email <addr> --name "<Name>" --password '<pass>' [options]

Options:
  --super           Grant isSuperAdmin (sees all events).
  --event <slug>    Grant this admin access to that event. Repeat for more.
  --role <R>        Role when granting events (default OWNER).
                      OWNER | ORGANIZER | VIEWER
  --name <string>   Display name (optional).`);
    process.exit(args.help ? 0 : 1);
  }
  if (args.role && !ROLES.has(args.role)) throw new Error(`Bad --role: ${args.role}`);

  const client = new Client({ connectionString: dbUrl() });
  await client.connect();

  const email = args.email.trim().toLowerCase();
  const hash = await bcrypt.hash(args.password, COST);

  const existing = await client.query(
    `SELECT "id" FROM "admin_user" WHERE lower("email") = $1`,
    [email],
  );

  let id;
  if (existing.rows.length === 0) {
    const ins = await client.query(
      `INSERT INTO "admin_user" ("id","email","name","passwordHash","isSuperAdmin","isActive","mustChangePassword","updatedAt")
       VALUES (gen_random_uuid(), $1, $2, $3, $4, true, true, now())
       RETURNING "id"`,
      [email, args.name || email, hash, args.super ? true : false],
    );
    id = ins.rows[0].id;
    console.log(`Created admin  ${email}`);
  } else {
    id = existing.rows[0].id;
    await client.query(
      `UPDATE "admin_user" SET "email"=$1, "name"=$2, "passwordHash"=$3, "isSuperAdmin"=$4, "updatedAt"=now() WHERE "id"=$5`,
      [email, args.name || email, hash, args.super ? true : false, id],
    );
    console.log(`Updated admin  ${email}  (password reset)`);
  }
  if (args.super) console.log("  isSuperAdmin  = true (sees every event)");

  if (args.events) {
    for (const slug of args.events) {
      const ev = await client.query(`SELECT "id" FROM "event" WHERE "slug" = $1`, [slug]);
      if (ev.rows.length === 0) {
        console.warn(`  ! event '${slug}' not found — skipped`);
        continue;
      }
      const eid = ev.rows[0].id;
      await client.query(
        `INSERT INTO "event_membership" ("id","adminId","eventId","role")
         VALUES (gen_random_uuid(), $1, $2, $3)
         ON CONFLICT ("adminId","eventId") DO UPDATE SET "role" = $3`,
        [id, eid, args.role],
      );
      console.log(`  granted ${args.role} on '${slug}'`);
    }
  }

  console.log("\nDone. The new admin can now log in at /login with that email + password.");
  if (args.super === false && !(args.events && args.events.length)) {
    console.log("Note: not a super admin and granted no events — they'll see an empty dashboard.");
  }
  await client.end();
}

main().catch((e) => { console.error(e.message); process.exit(1); });
