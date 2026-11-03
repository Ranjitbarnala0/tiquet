Download `Tiquet_1.6.0_x64-setup.exe` below and run it. Runs on Windows 10 and 11 (64-bit) and needs no administrator rights. Built and tested on a clean GitHub Windows machine, not on a developer's PC.

The installer is not code-signed yet: if Windows says "Windows protected your PC", click **More info** and then **Run anyway**.

**Upgrading from 1.3, 1.4 or 1.5:** Tiquet offers this version by itself; press **Actualizar ahora**. **From 1.0, 1.1 or 1.2:** run this installer once over the old version. Nothing has to be set up again.

New in 1.6:

- **The paper width follows the printer.** When a receipt printer is detected, plugged in, or picked in **Ajustes > Impresora**, Tiquet reads the paper its Windows driver is set up for: 58 mm rolls print 32 characters a line, 80 mm rolls 48. Tickets no longer come out broken on narrow paper. The "printer connected" message says the paper it found, and **Papel** still changes it by hand.
- **Simple tickets.** The shop's NIF is now optional. **Ajustes > Ticket > Datos del comercio en el ticket** turns the NIF, address, phone and "Le atendió" on or off. Without the NIF the ticket is titled **TICKET** instead of factura simplificada. Invoices with customer details still need the shop's NIF, and VERI\*FACTU always prints it.
- **Faster phone intake.** Brand buttons, a model list for each brand, and one-tap faults (Pantalla rota, Batería, No carga…).
- **Pickup receipt.** A repair handed back has **Justificante de entrega**: the device, the customer and a line to sign. The shop keeps it, together with the resguardo the customer hands back.
- **3 months to collect.** The default resguardo text says that the shop stops answering for a device not collected within 3 months of the notice. Shops that edited their own text keep it.
- **Price editing is visible.** In a sale, the price and the name of each line now look like buttons with a pencil, also on touch screens.
