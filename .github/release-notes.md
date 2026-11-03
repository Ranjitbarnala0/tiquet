Download `Tiquet_1.7.2_x64-setup.exe` below and run it. Runs on Windows 10 and 11 (64-bit) and needs no administrator rights. Built and tested on a clean GitHub Windows machine, not on a developer's PC.

The installer is not code-signed yet: if Windows says "Windows protected your PC", click **More info** and then **Run anyway**.

**Upgrading from 1.3 or later:** Tiquet offers this version by itself; press **Actualizar ahora**. **From 1.0, 1.1 or 1.2:** run this installer once over the old version. The database upgrades itself on first start, after a copy of it is kept.

New in 1.7.2: **tickets fit the paper, on their own.** (1.7.2 is 1.7.1 built again: Windows Defender mistook the 1.7.1 installer for a threat.)

- **58 mm printers print a proper ticket.** A till left on the old 80 mm setting sent 48-character lines to a 58 mm printer, which split every line in two. **Papel** is now **Automático**: Tiquet reads the paper from the printer's Windows driver and from its name (POS-58, XP-58, 58mm…), and when nothing says, it uses 58 mm, which prints right on any roll.
- **Tills already in use are put right by themselves.** The first time Tiquet opens after the update, it checks the printer again and says so: "Tickets ajustados al papel de …". A width someone chose by hand in Ajustes stays.
- **An 80 mm printer printing narrow tickets:** choose **80 mm** once in **Ajustes > Impresora > Papel**.
- **The test page** shows the paper Tiquet uses, and a row of numbers that must fit on one line.

Also in 1.7: the **Libro**. Type `funda 10` for a sale without a ticket or `-luz 45` for an expense, or press **Sin ticket** (F8) in Venta. It shows everything beside the ticket sales, with totals, delete with **Deshacer**, cash counted in **Caja**, and **Exportar** to Excel for the gestor.
