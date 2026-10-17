Download `Tiquet_1.1.0_x64-setup.exe` below and run it. Runs on Windows 10 and 11 (64-bit) and needs no administrator rights. Built and tested on a clean GitHub Windows machine, not on a developer's PC.

The installer is not code-signed yet: if Windows says "Windows protected your PC", click **More info** and then **Run anyway**.

**Upgrading from 1.0:** just run it. The sales database moves to `%LOCALAPPDATA%\TiquetData` (out of reach of the uninstaller), a copy is kept before the upgrade, and nothing has to be set up again.

New in 1.1:

- **Add anything by hand:** a big **+ Artículo** button (F4) and a card in the middle of an empty ticket. Name, price, quantity and VAT, and it can be saved as a product (and a quick button) on the way.
- **Change any line:** tap it to edit the name or description, price, quantity or VAT, and optionally the product's own price.
- **Products below the ticket:** Recientes, Favoritos and Todos, one tap to add. **Repetir** sells the items of an earlier ticket again. Products can be deleted (those already on tickets are archived instead).
- **Sets itself up on a new PC:** picks the receipt printer and paper width when it is clear which one it is, and starts in the language Windows uses. Everything can still be changed in Ajustes, where **Detectar** looks again.
- **Returns** (devoluciones) as rectifying simplified invoices with their own series, line by line, never more than was sold. Voids are for same-day mistakes only.
- **Full invoices** with the customer's name, NIF and address, in their own series. Sales over 3.000 € ask for them.
- **Cash drawer:** opening float, cash in and out, closing count against what the sales say, printed Z report.
- **Printing you can trust:** a ticket only counts as printed once Windows has let go of it. A printer that is off or out of paper gets its job cancelled, so nothing prints, and no drawer opens, hours later. Only the first print is the original; every other one says COPIA. Tickets that never printed are listed.
- **Training mode** for learning: own numbering, printed as not valid, never counted, can be wiped.
- **Forgotten admin password:** a recovery code is shown once at setup.
- **Data safety:** every committed sale survives a power cut. A restored backup never reuses a ticket number already handed out. A cart is never charged twice after a crash. The PC clock is checked. Backups can be restored from Ajustes.
- **Security:** the till locks itself when left unused, wrong passwords lock the account even across restarts, and admin passwords need 6 characters.
- **Sale screen:** a barcode scanned while charging adds the item instead of paying. Zero prices and impossible change are refused. The payment window fits 125-150 % laptop screens.
- **Admin:** CSV import that picks the sale price and keeps barcodes, unsaved-changes guards, price changes and discounts in reports, a VAT book (libro de IVA) export for the accountant.
