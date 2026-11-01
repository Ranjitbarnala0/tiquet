Download `Tiquet_1.5.0_x64-setup.exe` below and run it. Runs on Windows 10 and 11 (64-bit) and needs no administrator rights. Built and tested on a clean GitHub Windows machine, not on a developer's PC.

The installer is not code-signed yet: if Windows says "Windows protected your PC", click **More info** and then **Run anyway**.

**Upgrading from 1.3 or 1.4:** Tiquet offers this version by itself; press **Actualizar ahora**. **From 1.0, 1.1 or 1.2:** run this installer once over the old version. The database upgrades itself on first start, after a copy of it is kept, and nothing has to be set up again.

New in 1.5:

- **No more password every few minutes.** The till no longer locks itself unless someone chooses it in **Ajustes > Datos y copias**, and this update switches the old 10-minute lock off. Even when a lock is chosen, it never fires while someone is using the screen. **Al abrir Tiquet** can also sign a user in as Tiquet opens, with no password; **Bloquear** still asks for it.
- **Any printer.** **Ajustes > Impresora** now has four ways in:
  - **De tickets (USB):** the receipt printer, as before.
  - **Normal o PDF:** any printer installed in Windows (A4, label printers…) prints the ticket as a document through its own driver. With **Microsoft Print to PDF**, every ticket is saved as a PDF in `Documentos\Tiquet\PDF`, with no dialog.
  - **Red (IP):** as before.
  - **Bluetooth / COM:** Bluetooth receipt printers paired in Windows, and serial or USB-serial ones.
  
  Picking a printer from the list tells Tiquet which kind it is. Every printer installed in Windows is listed.
- **Plug and print.** A receipt printer plugged in later is taken on its own within seconds, also when Windows renames it after a change of USB port. A printer set to **Ninguna** on purpose stays that way.
- **Send the ticket.** **Enviar** on any ticket (in **Tickets**, and next to the last sale in **Venta**):
  - **WhatsApp:** opens the customer's chat (the WhatsApp app, or WhatsApp Web) with the ticket written in it; just press Send.
  - **Email:** with the shop's mail set up in the new **Ajustes > Enviar tickets** (Gmail, Outlook, Yahoo, IONOS or any SMTP server), it goes straight from Tiquet with the ticket as a PDF. Without it, the PC's mail app opens with the PDF ready to paste.
  - **Guardar PDF**, **Copiar PDF** (to paste with Ctrl+V in WhatsApp or an email) and **Copiar texto**.
- **Repairs on WhatsApp.** A repair still in the shop has a WhatsApp button that writes the right message for its status, for example "your phone is ready to collect", with the amount left to pay.
- **Fixed:** the repair form treated a network printer as no printer at all.

How it works: [README](https://github.com/Ranjitbarnala0/tiquet#readme). VERI\*FACTU is unchanged, and still off until it is switched on: [docs/verifactu.md](https://github.com/Ranjitbarnala0/tiquet/blob/main/docs/verifactu.md).
