Download `Tiquet_1.11.0_x64-setup.exe` below and run it. Runs on Windows 10 and 11 (64-bit) and needs no administrator rights. Built and tested on a clean GitHub Windows machine, not on a developer's PC.

The installer is not code-signed yet: if Windows says "Windows protected your PC", click **More info** and then **Run anyway**.

**Upgrading:** from 1.8.2 or later there is nothing to do, Tiquet updates itself when the till is free. **From 1.3 to 1.8.1:** Tiquet offers this version by itself; press **Actualizar ahora** once. **From 1.0, 1.1 or 1.2:** run this installer once over the old version. The database upgrades itself on first start, after a copy of it is kept.

New in 1.11.0: **several PCs in one shop, invoices on A4, and repairs that say what is paid.**

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
