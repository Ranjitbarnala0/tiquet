Download `Tiquet_1.14.3_x64-setup.exe` below and run it. Runs on Windows 10 and 11 (64-bit) and needs no administrator rights. Built and tested on a clean GitHub Windows machine, not on a developer's PC.

The installer is not code-signed yet: if Windows says "Windows protected your PC", click **More info** and then **Run anyway**.

**Upgrading:** from 1.8.2 or later there is nothing to do, Tiquet updates itself when the till is free. **From 1.3 to 1.8.1:** Tiquet offers this version by itself; press **Actualizar ahora** once. **From 1.0, 1.1 or 1.2:** run this installer once over the old version. The database upgrades itself on first start, after a copy of it is kept.

New in 1.14.3: **fixed reprint/copy gibberish on 58 mm thermal printers, dark crisp ink with thermal-safe dividers, and full codebase audit fixes.**

- **Fixed Copy / Reprint gibberish on 58 mm thermal printers.** Pure raster receipts now use a minimal clean ESC/POS raster header (`ESC @` + `ESC a 0` + `ESC J 0`, removing text-mode `ESC 2` / codepage commands before `GS v 0`) and safe 24-dot-row raster bands (`1,152` bytes/band on 58 mm) so reprinting copies, card receipts, repair receipts, and Z-reports never dump raw bytes as text.
- **Crisp dark ink and complete borders on 58 mm & 80 mm rolls.** Switched 203-dpi thermal typography to hinted 1-bit rendering (`NONANTIALIASED_QUALITY`) with heavier weights so small text and decimal figures print solid and sharp without broken strokes. Replaced solid full-width black fills and lines with thermal-safe geometric dashed rules, framed titles, and a symmetric **TOTAL** card staying under 46% black dots per row—eliminating faded badges and vanished horizontal rules on 58 mm print heads.
- **Full codebase audit fixes:**
  - Counter tickets with optional customer phone/name (no NIF) stay on the receipt printer, stay out of the **Facturas** list, and export as roll ticket PDFs rather than triggering A4 invoice routing.
  - **Facturas** ticket preview now applies the billing company identity (`bill: true`), recognizes a configured billing NIF when the shop NIF is empty, and formats optional customer details without dangling separators.
  - **Hoy** formats optional customer contact fields cleanly and routes **PrinterPick** by actual fiscal/bill status.
  - **Venta** keeps the **Cliente / Factura** chip active when "Emitir como factura" is toggled without customer fields.
  - Repair status badges (`PENDIENTE:`, `COBRAR`), the 58 mm width test ruler, narrow 58 mm header centering, A4 `headerNote`/`legalBottom` options, roll PDF height matching, and updater download banner/cleanup edge cases are all fixed.

Also in 1.14.2: **full shop & company customization, live ticket/A4 preview in Settings, and faster automatic updates.**

- **Complete Shop vs. Company configuration (`Ajustes › Comercio`).** Set a shop trade name, tagline/subtitle, legal company name on tickets/repairs, address, phone, email, website/social, and NIF/CIF—and separately configure the billing company for official invoices with three clear modes (`Empresa de facturación`, `Empresa + nombre tienda`, or `Siempre datos de tienda`). Includes a live **Ticket / Factura A4** preview switcher right next to the form.
- **Full ticket & repair header customization (`Ajustes › Ticket`, `Ajustes › Reparaciones`).** Control visibility of NIF, legal company name, tagline, address, phone, email, website, cashier, extra header banner text, and whether legal company + NIF sit below the shop name or at the bottom of the header block—plus whether repair receipts always display full company/NIF details.
- **Cleaner counter tickets vs. formal invoices in `Venta`.** Optional customer contact details on ordinary counter sales stay on your storefront shop header (`T` series), switching to the billing company only when issuing an invoice, entering a customer NIF, or printing on A4.
- **Faster, self-healing automatic updates.** Checks within 3 seconds of launch, every 5 minutes while running, and on window focus or network reconnection; retries failed downloads after 30 seconds; tracks idle time continuously; and never burns update retries on transient pre-exit errors.

Also in 1.14.0: **separate card machines on Venta, modern receipt typography, optional customer info and repair intake upgrades.**

- **Each card terminal on its own in Venta.** When the shop has more than one card machine, **Venta** shows each terminal in its own column and in its own total at the bottom, with each column's € total right in the header. **Ajustes › Venta** and **Ajustes › Máquinas** let you switch back to one combined **Tarjeta** stack anytime.
- **Crisp modern ticket design on the same roll.** USB and network receipt printers, Windows print drivers, PDFs and the on-screen preview now render smooth typography, solid rules, rounded badges and a framed **TOTAL** box at native 203 dpi on the same 58 mm or 80 mm paper (serial printers stay in text mode; **Ajustes › Ticket › Estilo de impresión** keeps the classic mode available).
- **Unified, zero-friction customer & billing details.** **Venta** now has a one-tap **Cliente / Factura (opcional)** chip right over the pay buttons (and in the charge window), with saved customer lookup and optional **Nombre**, **Teléfono**, **Email**, **Dirección** and **NIF**. Below the Spanish simplified limit (3.000 € retail / 400 € with a repair), optional customer fields stay in the normal ticket series (`T`) without forcing a NIF; entering a NIF (or passing the limit) issues a full invoice (`TF`) and optionally prints on A4. **Facturas** also accepts optional phone, email and non-NIF customer details under the limit.
- **Separate company details on invoices.** **Ajustes › Comercio** adds an optional **Datos distintos solo para facturas** section (name, legal name, NIF/CIF, address, phone, email) plus shop email. Counter tickets and repair receipts keep the storefront shop details, while bills and invoices use the billing company when filled.
- **Repair intake upgrades.** Repairs gain optional **DNI / NIE**, **Fecha aprox. de entrega**, **Contraseña / PIN del equipo** (always on the internal workshop tag, and optionally on the customer resguardo via **Ajustes › Reparaciones**), plus **Precios con IVA incluido** on priced receipts.
- **Refreshed dashboard & buttons.** Tactile, elevated pay buttons, color-accented columns with totals, and cleaner responsiveness.

Also in 1.13.4: **a bill with or without IVA, the A4 printer found by itself, and the whole till audited.**

- **A bill with IVA, or with nothing about it.** Each bill has a box, **Mostrar el IVA en la factura**. Unticked, the bill carries the items, their prices as typed and the total, and does not say "IVA incluido" or anything else about IVA, on A4 and on the roll. Ticked, it shows the base, the rate and the IVA of each rate, and prices can then be typed without IVA. A bill with no customer details starts unticked (**Ajustes › Facturas** can change that); one with name and NIF starts ticked, because a business needs its IVA, and the box can be unticked there too.
- **The same choice for tickets.** **Ajustes › Ticket › IVA en el ticket**: the breakdown, only the words "IVA incluido", or nothing.
- **Adding items is a button.** **Añadir artículo** under the lines, or Enter on a price. **Descuento** adds a line that takes off what is typed in it. A **Nota** on each bill (a warranty, the state of a used phone, the customer's name) prints on it.
- **The A4 printer is found by itself.** Tiquet takes Windows' default printer when it prints pages, or the only printer that does, if it is switched on, and says so. The "printers" of remote-support programs never count. Many USB inkjets and lasers were being taken for receipt printers and never offered for A4: fixed. A printer picked once for an A4 bill on a PC that had none is kept for the next ones. A bill sent to a printer that is switched off now says so, instead of waiting in its queue to come out later.
- **Better-looking paper.** A phone's IMEI on a line of its own, never cut in two. The total set apart between two rules. No "Cambio 0,00" when the exact money was given. On 58 mm paper the IVA table no longer runs its amounts together from 1.000 € up. Names such as Ionuț or Dănilă print with plain letters instead of "?".
- **A bill half typed is never lost.** It comes back after an update, a restart or a power cut.
- **The whole till, audited.** Four independent reviews of every part found these, all fixed:
  - Items scanned while a sale was being charged vanished from the next ticket.
  - A quantity typed in a line and then **Efectivo**, without leaving the box, charged the old quantity.
  - Typing a product's whole name ending in a number ("Funda iPhone 13") and Enter added a free item at 13 €.
  - Cashiers could not open **Facturas**.
  - **Gasto** took "Pantalla iPhone 11" as an expense of 11 € at the first Enter.
  - A return could take an item back and leave its discount on the sale, paying out more than was paid.
  - On a second PC, a sale rung after a lost answer could be answered "ya estaba cobrada" and not saved.
  - The same recharge or expense sent again after a lost answer was saved twice.
  - A sale corrected after the till had been counted and opened again moved into the afternoon's cash.
  - The cash drawer stayed shut when a ticket that failed to print was printed with **Reintentar**.
  - A long ticket sent by WhatsApp could arrive cut off before its total.
  - A second PC still opened whatever link the main till's answer named, and kept its sign-in after **Bloquear** when the main till was off.
  - The VAT book counted the amounts of voided and corrected tickets.
- **1.13.4 over 1.13.1,** from three more reviews of the fixes, for a second PC whose answer from the main till was lost on the WiFi: a ticket emptied with **Vaciar** and brought back with **Deshacer** could be charged a second time, and a ticket emptied line by line kept the old sale's number, so the next customer's identical sale was answered "ya estaba cobrada" and not saved. Now the first line on an empty ticket always starts a sale of its own, and if the earlier one had in fact been saved, Tiquet says so by its number right after the charge (**Antes se guardó T… : si sobra, bórrala**): nothing is paid twice or kept twice without anyone knowing, also when the ticket was emptied while the charge was still waiting, or Tiquet was restarted in between. A ticket that comes back after a restart is emptied as "ya se había cobrado" only when it is still exactly that sale: with something added since, it stays to be charged. **Deshacer** on the last sale puts its items back as a sale of its own. In **Facturas**, a **Descuento** line stays a discount whatever it is called, and a bill that came back half typed sets its IVA box by the kind of bill again. A bill printed again from **Emitidas** that failed now says why (the printer is off, out of paper) instead of a bare code.

Also new in 1.12.1: **invoices made your way, repairs that ask for nothing, and a printer for each print.**

- **Facturas, redone.** The page opens ready to work: the invoice on the left, and on the right the page exactly as it will print, changing as you type. The total and the button are always at the bottom. Next to the button it says what is still missing, if anything, and pressing it takes you there: the button never stays grey without saying why. A half-made invoice waits while you look at another screen.
- **With or without the customer's details.** **Sin datos** is a simplified invoice, for a cover or a cable: it asks the customer for nothing. **Con nombre y NIF** is a full invoice, for a phone or a business; earlier customers come up as you type their name.
- **VAT with nothing to think about.** Prices are typed with VAT included, and 21% is already there. A line can take another rate, or 0%, or be a discount, and prices can be typed without VAT instead. A phone's IMEI has its own box right under the model, wide enough for all of it, and can stay empty.
- **A4 or ticket, on the printer you choose.** Each invoice can come out on an A4 page or as a ticket, on any printer of the PC, just for that print. By itself an invoice comes out on A4 when the PC has an A4 printer, and as a ticket when it has none. Tickets from Venta keep going to the receipt printer on their own. **Hoy** can reprint anything on another printer too.
- **Invoices can be corrected.** A wrong model or name? **Editar** on the invoice (in **Facturas** or in **Hoy**), change what is wrong and save. The new one prints clean, with nothing on it saying it was corrected, and keeps the date of the sale, so no day's cash moves. Tiquet keeps the old one, marked **Corregida**, with the invoice that replaced it and who did it. The owner can correct any day of this year unless VERI\*FACTU is on; cashiers, what they did today if **Ajustes** lets them. A return, or a sale that handed back a repair, is still put right with a return in **Hoy**.
- **Everything about invoices in one place: Ajustes > Facturas.** A4 or ticket, the A4 printer, prices with or without VAT, the IMEI box, the VAT table on simple invoices (without it they say "IVA incluido") and the text at the bottom, with a sample invoice beside them that changes as you go.
- **Repairs ask for nothing.** Name, phone, model and fault can all stay empty: a customer who gives no name still gets a resguardo, and its number is enough to find the phone. **Ajustes > Reparaciones** says which of them must be filled in (those then say **obligatorio**), and shows the resguardo as the customer will get it.

Also new in 1.11.0: **several PCs in one shop, invoices on A4, and repairs that say what is paid.**

- **Several PCs, one shop.** In **Ajustes > Cajas** turn on **Compartir esta caja**: this till keeps the data for all of them. On another PC of the shop, on the same WiFi, install Tiquet and press **¿La tienda ya tiene Tiquet en otro PC?** on its first screen: it finds the till by itself. Each person signs in with their own user and password and sells into the same day, the same numbering, the same drawer and the same **Hoy**. Each PC prints on its own printer, with its own paper. If the main till is switched off, the others say so, keep the ticket on the screen and carry on as soon as it is back.
- **Facturas**, a new section on the left. **Nueva factura**: the customer, what is sold (a phone with its IMEI on the same line), how it is paid, and it comes out on an A4 page by itself. Each PC chooses its A4 printer in **Ajustes > Impresora**; without one, invoices print as before. An invoice made in **Venta** with **Factura** comes out on A4 too. Its PDF is the A4 page, kept in `Documentos\Tiquet\Facturas`, and that is what goes by email.
- **Repairs say what is paid.** Every repair in the list shows **PAGADO**, **FALTA 50,00 €**, **SIN PAGAR 80,00 €** or **SIN PRECIO** in coloured letters, and the repair says it again at the top. **Sin cobrar** and **Pagadas** list just those. Paid in full when the phone is left: one tap on **Lo paga todo ahora**. The resguardo and the label on the phone print PAGADO, or what is left to charge, in big letters.
- **The main page no longer keeps the PC busy.** Since 1.10.0, with nothing on the ticket, the page kept redrawing itself without end and used the processor for nothing. Fixed.
- **Licence check.** Tiquet tells its supplier's server which PC it runs on, at start and every few hours: the shop's name and NIF, the PC's name, the version and how many tickets there are. Nothing about customers or what was sold. It lets the supplier push an update and control copies. Until that server is published it does nothing.

Also new in 1.10.0: **a new main page, where nothing gets added by accident.**

- **No more product buttons.** Recientes, Favoritos and Todos are gone. Items come in through the box, now at the bottom, with the day's totals right above it. The right side is the same.
- **The day stacks up in their place:** columns for cash, card, Bizum, recharges and expenses, the newest on top. They only show: nothing there can be tapped.
- **What you type as money is money.** Typing 10 used to select the first product with a 10 in its name, at its own price. Now it is 10,00 €.
- **Nothing is selected for you.** A word that fits several products selects none until you choose. The list opens upwards, the best match next to the box.
- **A second Enter does nothing.** It used to open the charge window. Holding Enter, or a scanner sending it twice, is the same.
- **Numpad + no longer changes a quantity** without anyone seeing, and **Recarga no longer starts with the amount of the ticket** and then empties it.
- **A forgotten ticket is marked.** After 10 minutes without being touched its lines turn amber, and what is added next stays apart. **Quitar** removes only those lines.
- **Charged by mistake?** **Deshacer**, right after the sale, deletes it and puts its items back on the ticket.
- **A barcode scanned with the quantity box selected** goes to the box, not into the quantity.

Also new since 1.9.1: **everything stacks by time, and Hoy keeps things apart.**

- **On the ticket the last thing added is on top.** Adding the same item again moves its line to the top. The printed ticket keeps the order things were added.
- **Products: the last added first,** in **Todos**, **Favoritos** and **Productos**. **Ajustes > Venta** can make them alphabetical.
- **Hoy in separate stacks:** Efectivo, each card machine, Bizum, Recargas and Gastos, each with its total and the newest on top. **Por hora** shows one list by time. Tap a figure at the top to see only its lines.

Also in 1.9.1: **a ticket on the screen no longer stops an update.** Until now Tiquet did not even look for a new version while something was on the ticket, and a ticket left there stays after closing Tiquet, so a till could go without updates and say nothing. Now it always looks and downloads, the blue bar always shows, and a ticket forgotten on the screen comes back after the update. The version is shown at the bottom left.

Also new in 1.9: **your machines, and you decide what counts as an expense.**

- **A recharge is not always an expense.** In cash it counts as a recharge and nothing more. Made on a machine it also counts as an expense, less the commission. Both can be changed. Recharges saved before this version stay as they were; **Editar** in **Hoy** applies the new rule to one.
- **Name your machines.** **Ajustes > Máquinas**: one or many, card terminals and recharge machines, with the names you want. There is always one, called Datáfono until you rename it.
- **Which machine?** With one, Tiquet asks nothing. With several, **Tarjeta** asks which one with big buttons right over the pay buttons (a tap, a number or Enter), and the recharge window shows each of them. You choose the default for each, or turn the question off.
- **Each machine has its own rules.** Whether it takes cards, whether it does recharges, whether they count as an expense and its commission, how a recharge customer pays, and whether what it takes in sales counts as an expense: all of it, or a percentage such as the bank's fee.
- **Totals per machine.** In **Hoy** (tap one to see only its lines), **Informes**, the printed summary, the closing report and the file for the gestor.
- **Wrong machine?** Tap the right one after the sale or in **Hoy**, like the way of paying.
- **More in Ajustes.** Turn off what the shop does not use: the Gasto and recharge buttons, **Reparaciones**, each total at the bottom of the page. Set the usual purchases as buttons, how they are usually paid, the usual till float, a slip with every recharge, and whether cashiers can give returns.
