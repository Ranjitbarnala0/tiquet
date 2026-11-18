# Working on Tiquet

For any AI agent or developer who changes this code. [README.md](README.md) is for the people who use Tiquet; this file is for whoever changes it. Read it all before the first change.

## 1. What matters

- Tiquet is the till of small phone shops in Spain: sales and receipts on thermal printers, invoices on A4, repairs, recharges, expenses, the day's cash. Windows only, offline, one SQLite file.
- Real shops take money with it every day, and every installed copy updates itself from this repository's releases. A mistake released here reaches a shop counter within hours. Correct tickets and data come before features.
- The owner judges Tiquet at a busy counter. Most of his feedback is about friction: an extra click, a question, a refusal, a button that does something by surprise.

## 2. Rules

Each of these was learned the hard way.

**Code and tools**

- **Never run `cargo fmt` or Prettier.** The code is formatted by hand (long lines; there is no rustfmt.toml or Prettier config). `cargo fmt` once rewrote 31 files and buried the real change. Match the lines around you.
- **No window ever appears by surprise.** Every process Tiquet or a helper script starts is hidden (`windowsHide: true` in Node, `CREATE_NO_WINDOW` in Rust and Python), and the installer runs silent. When testing on someone's PC, start programs only from your own shell: never `Start-Process`, `cmd /c start`, a `.vbs` or a scheduled task. In Git Bash an argument like `/S` becomes a path: use `MSYS_NO_PATHCONV=1` or PowerShell, or the installer's wizard opens on the screen.
- **No real shop or customer data in this public repository.** Demo data is "Mi Tienda Móvil", NIF `B12345674` (`B12345678` fails the checksum).
- **No secrets in the repository.** The updater's signing key lives outside it and in the repository secrets `TAURI_SIGNING_PRIVATE_KEY` / `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`. If that key is lost, installed copies can never update again.

**Product**

- **Decide; don't ask.** The owner does not want questions, least of all legal or tax ones. Pick the simplest option that is legally sound, build it completely, and write the decision and its reasons into the docs, as [docs/verifactu.md](docs/verifactu.md) does. Where only he can supply something (his NIF, a purchase), leave a placeholder that fails safe and say so.
- **Convenient by default, strict by choice.** The convenient behaviour is the default; the strict one is a setting. When a printer or a channel fails, use the next way that works instead of showing an error. Legal guarantees are not friction and stay: numbering without gaps, issued records never edited or erased, VERI\*FACTU.
- **One main page.** Selling, recharges and expenses happen in **Venta**. A new capability is a switch, or a part of a screen that exists, not a new screen. Big buttons near the hand; typing anywhere lands in the box.
- **Nothing happens by itself.** No product buttons near the hand, no preselected guess that could be wrong, no second Enter that charges, no effect on a line that is only selected. When one accident is reported, fix its whole family.
- **Lists stack by time, newest on top**, with cash, card and each machine apart. Test with a busy day, not with three lines.
- **Plain words; no unexplained grey buttons.** Spanish first (`tr('español', 'English')`), the shop's own words, no accounting terms. A button is never disabled without saying why: show what is missing beside it, and let a press take the user there.
- **Nothing is forced on the customer.** A simplified invoice needs no customer data, and every repair field is optional. What a shop wants required is a setting (**Ajustes › Reparaciones**).

## 3. Map

Core, `src-tauri/src`:

| File | What it does |
| --- | --- |
| `api.rs` | Every command the UI can call: `dispatch(state, "area.verb", args)` returns JSON. Checks the role with `st.user()?` / `st.admin()?` and catches panics, so one bad handler never takes the till down. |
| `tickets.rs` | Sales, numbering, voids, returns, corrections (`replaces`), the Facturas list, earlier customers. |
| `db.rs` | Schema, migrations (`MIGRATIONS`, one entry per version) and the triggers that keep issued records unchanged. |
| `settings.rs` | `Settings` (every field has a default), `clean()`, `DEVICE_KEYS` (what each PC keeps for itself), `upgrade()` for one-time fixes. |
| `layout.rs`, `escpos.rs`, `codepage.rs` | A ticket as fixed-width lines; the same lines feed the preview on screen and the ESC/POS bytes. |
| `printer.rs` | Where a print goes (Windows queue as raw data, GDI "document", network port 9100, COM port), print jobs, paper width, the printer picked for one print. |
| `invoice.rs` | A4 invoices as `Sheet`s of `Mark`s in millimetres. The PDF and the GDI print come from the same sheets, and the screen draws them as SVG (`src/Sheets.tsx`). |
| `repairs.rs` | Repairs (series `REP`), deposits, delivery through a line in Venta. |
| `book.rs`, `cash.rs`, `reports.rs` | Expenses and recharges; the drawer, float and closing count; totals and exports. |
| `verifactu.rs`, `aeat.rs` | VERI\*FACTU records, their chain, the AEAT. See [docs/verifactu.md](docs/verifactu.md). |
| `lan.rs` | Several PCs of one shop: the main till serves `dispatch` to the others. |
| `license.rs`, `update.rs` | Check-in with the supplier's control server; self-update. |
| `share.rs`, `pdf.rs`, `mail.rs` | WhatsApp text, the roll-sized PDF, SMTP. |
| `app.rs` | The window and the command-line flags (`--selftest`, `--check`, `--no-window`, `--data`). |
| `../examples/devserver.rs` | The same `dispatch` over HTTP, for working in a browser. |

UI, `src`: `Shell.tsx` (screens, settings, updates), `Sale.tsx` (Venta), `Board.tsx` (the day's columns on Venta), `Quick.tsx` (recharge and expense windows), `Day.tsx` (Hoy, Caja, Informes), `Invoices.tsx` (Facturas), `Repairs.tsx`, `SettingsScreen.tsx` (Ajustes), `PrinterPick.tsx`, `Paper.tsx` and `Sheets.tsx` (previews), `api.ts` (`call()`), `i18n.ts` (texts, error and warning codes), `types.ts`.

## 4. How it works: the parts that bite

- **The core owns money.** Amounts are integer cents, VAT is in basis points (`2100` = 21 %), and prices include VAT. The UI may show a sum while typing, but it sends `expectedCents` and the core refuses a different total (`E_TOTAL_CHANGED`).
- **Errors are codes.** The core fails with `E_SOMETHING` (or `E_SOMETHING:detail`), and `src/i18n.ts` turns each into Spanish and English. Something that worked with a caveat returns a warning `W_…` instead. Add the text whenever you add a code.
- **Issued means unchangeable.** Tickets, their lines and VAT rows, cash moves, repair events and fiscal records are protected by SQLite triggers once sealed. "Delete" is a mark (who, when, why), never an erasure (Ley 11/2021). The only real deletion is wiping practice (training) data.
- **Numbers without gaps.** Series are prefix + tag + year (`T2026-000001`; tags `F` invoice with customer, `R` return, `P` practice). Each number is the series' highest plus one, never below `series_floor`, and `numeracion.json` beside the database makes sure a restored backup never hands out a number twice.
- **What a paper says about VAT** is kept with each ticket (`ShopInfo.vat`, a `VatView`): `full` (base, rate and VAT of each rate, and each price without VAT), `note` (the words "IVA incluido" and the rate) or `none` (items, prices, total). A bill made in Facturas is `full` or `none`, by the box on it (`showVat`; the default is `invoiceShowVat`). A receipt of Venta follows **Ajustes › Ticket** (`show_vat` + `vat_note`), and an invoice with a customer made in Venta is `full`. Tickets from before 1.13 have no `vat` and print as they always did (`ShopInfo::vat_view`). The amounts and the VAT rows in the database are the same in all three: only the paper changes.
- **A sale id used again** for another cart (`E_SALE_REUSED`) is a new sale whose screen kept the id of one it never heard was saved: the screen takes a new id and charges it. The same cart sent twice gets its ticket back.
- **Corrections** (`sale.finalize` with `replaces`) issue the new ticket and void the old one ("Corregida en …") in one transaction.
  - Allowed only on a live sale of the same mode (practice or real), with no repair line, from this year (`tickets::fixable`).
  - `void_in` then checks the day and that there are no live returns: cashiers may correct their own sales of today, when Ajustes lets them; the owner any day while VERI\*FACTU is off.
  - A correction made the same day is issued now, unless the till was opened or counted since the sale: then, and for an earlier day, it keeps the old ticket's date and time (`correction_time`), so no till count and no day's cash changes.
  - The new paper carries no mark. The trail is in the database (`replaces`, `replaced_by`).
  - A ticket with a customer or the `bill` flag is corrected in Facturas, never in Venta, which would drop the customer.
- **Settings** are one JSON in the `settings` table. Every field has a default, so an old database still loads. `DEVICE_KEYS` (printer, paper, A4 printer, invoice format, language…) stay on each PC. A screen may meet settings sent by an older main till: read new fields with a fallback (`settings.x ?? ''`).
- **Printing.**
  - `printer::target` picks the queue, document, network or serial path. `Settings.one_off` (never saved) is a printer picked for one print, and it wins; `OneOff::over` lays the print out for that printer's own paper width.
  - Which printer is which: a name, driver or port that says receipt makes a receipt printer (`score`). A USB port alone is a weak sign, so a printer whose driver prints pages (180 mm or wider) is an office printer after all (`Entry.page`); until 1.13 such inkjets and lasers were never offered for A4.
  - `printers.auto`, polled by the screen, sets up the receipt printer and, since 1.13, the A4 printer (`a4_auto`: Windows' default when it prints pages, or the only one that does; never when "None" was chosen on purpose, key `invoicePrinterOff`). A printer picked for an A4 bill on a PC with none is kept (`printers.a4`).
  - The cash drawer always opens on the receipt printer, whatever printer was picked.
  - Paper width is read from the driver (`paper_auto`); unknown means 32 columns, which prints right on any roll.
  - An invoice asked for on A4 on a PC without an A4 printer comes out as a ticket, with `W_A4_AS_TICKET` (`W_A4_GONE` when the printer set up is no longer in Windows). An A4 printer Windows knows is off or paused takes nothing (`E_PRINT:offline`): the page would otherwise wait in its queue.
  - A print counts as printed only when Windows or the printer has taken all of it. Otherwise the job is cancelled, so no drawer opens later on its own.
- **Several PCs** (`lan.rs`).
  - The main till answers on port 47713, to private addresses only, and only requests with the header `X-Tiquet: 1`. Each PC has its own sign-in.
  - A request from another PC runs with that PC's device settings, and what prints for it goes back and prints on its own printer.
  - `lan::local` names the commands that always run on the PC itself (`printers.*`, `lan.*`, `license.*`…).
- **Updates** (`update.rs`, `src/Update.tsx`).
  - Checked every 15 minutes, downloaded in the background, installed only when the till is free (nothing being charged, no dialog open, 2 minutes untouched). The same user is signed back in.
  - Nothing may hold back the check: up to 1.9.0, a ticket left on the screen stopped it, and tills went days without updates.
  - Command-line arguments survive the relaunch; environment variables do not.
- **Licence** (`license.rs`): reports to the control server named in `control.json` (repository root, read from GitHub) at start and every 6 hours, and checks the answer's Ed25519 signature. A copy that is locked, or not heard from for 45 days (14 for a new PC), still shows, prints and exports everything but takes no money.
- **VERI\*FACTU** is off by default. When it is on, voids and corrections are of today only. Details in [docs/verifactu.md](docs/verifactu.md).

## 5. Develop and check

Requirements: Windows 10/11, Rust (stable, MSVC), Node.js 20, Visual Studio Build Tools (C++).

```
npm ci
npm run devserver      # the core on http://127.0.0.1:1430, data in ./devdata
npm run dev            # the UI on http://localhost:1420, /api proxied to the devserver
```

- In a browser, sign in with a POST to `/api/login` `{ "userId": 1, "password": "…" }`, or through the screen.
- Stop the devserver before rebuilding it: Windows locks a running exe.
- After any UI change, read the browser console. An endless re-render of the empty Venta page (1.10.0) showed up only there.
- `CARGO_TARGET_DIR` can point to another disk; a debug target is about 6 GB.

Before every commit:

```
cd src-tauri
cargo test --lib
cargo clippy --all-targets --features devserver -- -D warnings
cd ..
npx tsc --noEmit -p .
npx vite build
```

`cargo test --lib -- --ignored` also pushes jobs through the Windows spooler. Every push runs the same checks on GitHub ([ci.yml](.github/workflows/ci.yml)).

- **A test proves a rule better than a click.** Add one in the module you change. Example: `a_bill_is_corrected_with_a_clean_new_one_and_the_old_keeps_why` in `tickets.rs`.
- **Look at the packaged app, not only the dev page.** Run a built or installed `tiquet.exe --no-window --data <copy of a data folder>` with `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=9333`, then drive it with [tools/cdp-eval.mjs](tools/cdp-eval.mjs) and [tools/cdp-shot.mjs](tools/cdp-shot.mjs). Item fields too narrow at 1366 px showed up only there (1.12.0).
- **Have a second reader review a big change for bugs before release.** An independent review of the 1.12.0 diff found eleven real bugs, and four reviews of the whole of 1.12.1 (money, printing, screens, several PCs) found about forty more.

Environment switches:

| Variable | Effect |
| --- | --- |
| `TIQUET_PRINTER=spooler` | The devserver prints for real (otherwise `.bin` files in `<data>/prints`). |
| `TIQUET_UPDATE_URL` | Another `latest.json`, to test an update before it is public. |
| `TIQUET_CONTROL_URL` | Another control server, e.g. `npx wrangler dev`. |
| `TIQUET_LAN_BIND` | Address the main till listens on (`127.0.0.1` in tests; `0.0.0.0` makes Windows Firewall ask). |
| `TIQUET_API` | For `npm run dev`: a second devserver, to try two PCs side by side. |
| `TIQUET_DOCUMENTS` | Where files are saved instead of `Documents\Tiquet`. |
| `TIQUET_INVOICE_GDI`, `TIQUET_DOC_OUT` | Output files for the ignored tests that print through the Windows PDF writer. |

## 6. Release

1. Set the version in `package.json`, `package-lock.json` (two places), `src-tauri/Cargo.toml`, `src-tauri/Cargo.lock` (the `tiquet` entry) and `src-tauri/tauri.conf.json`. The workflow refuses a tag that does not match.
2. Write the top of [.github/release-notes.md](.github/release-notes.md) (it becomes the release text) and update the README if behaviour changed.
3. Commit as "Tiquet X.Y.Z: what the shop gets", tag `vX.Y.Z`, push `main` and the tag.
4. [release.yml](.github/workflows/release.yml) builds on a clean Windows machine. It runs the tests and the packaged app's `--selftest`, signs the installer for the updater, writes `latest.json` and creates a **draft**.
5. Make it a pre-release, not the latest: `gh release edit vX.Y.Z --draft=false --prerelease --latest=false`. Tills follow `releases/latest`, so they keep the old version for now.
6. Run a real update test. Take an installed previous version, run it hidden on a copy of a data folder ([tools/prep-update-data.py](tools/prep-update-data.py)) with `TIQUET_UPDATE_URL=https://github.com/<owner>/<repo>/releases/download/vX.Y.Z/latest.json`. Its log should show the download, the install after about 2 idle minutes, and the new version starting. On a schema change, the database is copied to `migraciones` first.
7. Windows Defender sometimes flags an unsigned NSIS installer minutes after it appears (`Trojan:Win32/Bearfoos.A!ml`, a false positive). Download the exact installer and run [tools/soak.ps1](tools/soak.ps1): 14 fresh reads, 45 s apart, then the detections. A flagged build is never published: bump the version and build again.
8. Publish with `gh release edit vX.Y.Z --prerelease=false --latest`, then check that `https://github.com/<owner>/<repo>/releases/latest/download/latest.json` names the new version.
9. If a pre-release turns out wrong, it stays a pre-release and the fix ships as the next version. Never move a tag that was pushed.

## 7. Lessons

| When | What happened | What it left |
| --- | --- | --- |
| 1.0 | `setup.exe /S` from Git Bash opened the installer's wizard on the owner's screen | `MSYS_NO_PATHCONV=1`; check how any program will start before starting it |
| 1.6 | A 58 mm printer printed 48-column tickets in a real shop | Paper width from the driver; 32 columns when unknown |
| 1.7.1 | Defender flagged the installer after it was published | Draft, soak, then publish |
| 1.8.0 | A separate "Libro" screen was rejected | One main page |
| 1.9.0 | Tills with a ticket on the screen never looked for updates | Nothing holds back the update check |
| 1.10.0 | The empty Venta page re-rendered forever and kept the CPU busy | Read the console; measure an idle page with [tools/cdp-idle.mjs](tools/cdp-idle.mjs) |
| 1.10.0 | Product buttons added items by accident | Nothing near the hand acts by itself |
| 1.11.0 | `cargo fmt` rewrote 31 files | Never format |
| 1.12.0 | Review before release: practice mode could void real bills; a return could be "corrected" into a sale; a repair could reopen; prices drifted a cent when switching VAT modes; the drawer could open on the picked printer; Editar in Hoy dropped the customer; a picked A4 printer was refused; correcting an earlier day moved its cash into today | The rules in section 4 |
| 1.12.0 | Item and IMEI fields too narrow at 1366 px, seen only in the packaged app | Look at the packaged screens before publishing |
| 1.12.1 | Shops asked for a bill with no mention of VAT; it had been asked for before and not built | Build what the shop asks for, as an option, and say so |
| 1.12.1 | Bills kept printing as tickets: the A4 printer had to be found in Ajustes, and USB office printers were classed as receipt printers | Set up by itself whatever can be worked out; never make the shop configure the obvious |
| 1.12.1 | Audit of the whole app: items scanned during a charge vanished; a quantity typed and then Efectivo charged the old one; "Funda iPhone 13" + Enter added a free item at 13 €; cashiers could not open Facturas; a return left its discount behind; a lost answer on a second PC lost the next sale | Sections 2 and 4; audit the whole app now and then, not only the diff |

## 8. Still open

- Never tried on a real receipt or A4 printer on the development PC. At a shop, the first step is the test page in **Ajustes › Impresora** and the A4 test in **Ajustes › Facturas**.
- The installer is not code-signed: Windows SmartScreen asks "More info › Run anyway", and Defender can still flag a build.
- VERI\*FACTU: the producer's details are placeholders and no real record has been sent yet ([docs/verifactu.md](docs/verifactu.md)).
- Repairs: one deposit ticket per repair, at most 400 €; paying the rest before pickup is not built.
- Used phones under the margin scheme (REBU): a bill can now leave VAT out of its paper, but the books still count it at the line's rate; the margin scheme itself is not built.
- From the 1.12.1 audit, known and not yet fixed:
  - A second PC follows the main till to a new address by its install id alone; a device on the shop WiFi that copies that id could draw its sign-ins. It should prove it knows the PC's token first.
  - A database restored onto another PC keeps its install id: that PC cannot join the first as a second PC, and both may answer as the main till.
  - A restore brings back the copy's own sharing and licence rows, and other PCs stay signed in until the main till restarts.
  - The "database will not open" screen only lists backups from the default folder.
  - Any device on the WiFi can lock sign-in with wrong passwords; a copy with its `controlUrl` row removed is not judged by the licence.
  - Practice mode still opens, moves and closes the real till and takes in real repairs.
  - A return or a cash move sent again after a lost answer is saved twice (recharges, expenses and sales are safe).
  - A reprinted till close recomputes its parts but keeps the stored expected amount.
  - A4 and "document" prints count as printed once Windows has spooled them: a printer that Windows believes is on, but is not, still queues the page.
