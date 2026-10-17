# Tiquet

Point-of-sale ticket printing for small shops. A Windows desktop app that prints receipts on ESC/POS thermal printers (80 mm and 58 mm), works fully offline and keeps every sale in a local database.

Built for a phone accessories shop in Spain that was printing tickets from a 1990s-style program with an advert in the footer. Tiquet prints the same kind of ticket, with the shop's own logo and none of the noise.

![Sale screen with the live ticket preview](docs/venta.png)

| Charging | Tickets |
| --- | --- |
| ![Payment with change and the final ticket](docs/cobrar.png) | ![Ticket history with reprint and void](docs/tickets.png) |
| **Reports** | **Settings** |
| ![Sales, payment methods and VAT](docs/informes.png) | ![Shop, logo and printer settings](docs/ajustes.png) |

## Install

1. Download `Tiquet_1.0.0_x64-setup.exe` from [Releases](../../releases/latest).
2. Run it. The installer is not code-signed yet, so Windows SmartScreen may say "Windows protected your PC": click **More info**, then **Run anyway**.
3. On first start, enter the shop details and create the administrator account.
4. **Ajustes > Impresora**: pick the ticket printer and press **Imprimir página de prueba**.

The printer must be installed in Windows (with the manufacturer's driver or "Generic / Text Only"). If accents print as strange symbols, press **Prueba de caracteres** and choose the line that reads correctly.

## What it does

- **Selling at keyboard speed.** One search box for everything: type a name, scan a barcode, `3*cable` for three units, `funda 12,50` for an item at any price, or just `12,50`. Enter adds, F12 charges, Ctrl+Z undoes.
- **Exact preview.** The ticket on the right is rendered from the same lines that are sent to the printer, so what you see is what prints, including change and VAT.
- **Payments.** Cash with quick amounts and change, card, Bizum. Opens the cash drawer on cash sales.
- **Tickets that cannot be edited.** Numbered per year without gaps (`T2026-000001`). Issued tickets can be reprinted as a copy or voided with a reason, never edited or deleted; the database enforces it.
- **Admin.** Products (with CSV import from any spreadsheet), users with roles, sales reports by day, payment method and VAT rate, a printable daily summary, CSV export for the accountant.
- **Shop identity.** Logo or photo on the ticket (converted to black and white for thermal paper), header, footer, numbering prefix.
- **Never loses a sale.** The sale is stored before printing; if the printer is off, the ticket can be printed again. The cart survives a crash or power cut. A database backup is taken every day.
- **Spanish and English** interface. Tickets always print in Spanish.

## Keyboard

| Key | Action |
| --- | --- |
| Enter | Add the highlighted result; on an empty search, charge |
| F12 | Charge |
| F1 / F2 / F3 | Cash / card / Bizum while charging |
| Enter / Shift+Enter | Print the ticket / save without printing |
| Ctrl+Z | Undo the last change to the ticket |
| Numpad + / - | Change the quantity of the selected line |
| Delete | Remove the selected line |
| F2 | Focus the search box |

## Where the data lives

- Database: `%LOCALAPPDATA%\es.tiquet.caja\tiquet.db` (SQLite).
- Backups: `Documents\Tiquet\Copias de seguridad`, one per day, last 60 kept. The folder can be changed in Ajustes.
- Exports: `Documents\Tiquet\Exportaciones`.

To restore a backup, close Tiquet, delete `tiquet.db-wal` and `tiquet.db-shm` if they exist, and copy the backup file over `tiquet.db`.

## Build from source

Requirements: Windows 10/11, [Rust](https://rustup.rs) (stable, MSVC), Node.js 20 or newer, Visual Studio Build Tools with the C++ workload.

```
npm install
npm run tauri build
```

The installer is written to `src-tauri/target/release/bundle/nsis/`.

For UI work without the desktop shell, run the core over HTTP and open the Vite dev server in a browser:

```
npm run devserver      # Rust command layer on http://127.0.0.1:1430
npm run dev            # UI on http://localhost:1420, proxies /api to the devserver
```

Tests: `cargo test` in `src-tauri` (add `-- --ignored` to also push a job through the Windows print spooler).

## How it is built

- **Tauri 2** shell, **Rust** core, **React** UI, **SQLite** storage. It runs on the WebView2 runtime that ships with Windows 11; on an older Windows 10 the installer fetches it.
- The UI never computes money. Totals, VAT and the ticket layout come from the core, which renders the ticket once into fixed-width lines; the same lines feed the on-screen preview and the ESC/POS encoder.
- Printing goes straight to the Windows spooler as a raw job (`RAW` for classic drivers, `XPS_PASS` for v4 drivers), with code page selection, Chinese-mode cancel for generic printers, logo raster, cut and drawer pulse.
- Passwords are hashed with Argon2id. Five wrong attempts lock that user for 30 seconds. Roles are checked in the core, not only in the UI.

## Not in this version

Spanish anti-fraud billing rules (VERI\*FACTU, Real Decreto 1007/2023) require hash-chained invoice records and, in VERI\*FACTU mode, sending them to the AEAT. That is planned as a separate module once the shop's tax regime is confirmed; some regimes, such as recargo de equivalencia, are outside its scope.
