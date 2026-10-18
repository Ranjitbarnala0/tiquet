Download `Tiquet_1.4.0_x64-setup.exe` below and run it. Runs on Windows 10 and 11 (64-bit) and needs no administrator rights. Built and tested on a clean GitHub Windows machine, not on a developer's PC.

The installer is not code-signed yet: if Windows says "Windows protected your PC", click **More info** and then **Run anyway**.

**Upgrading from 1.3:** Tiquet offers this version by itself; press **Actualizar ahora**. **From 1.0, 1.1 or 1.2:** run this installer once over the old version. The database upgrades itself on first start, after a copy of it is kept, and nothing has to be set up again.

New in 1.4:

- **Repairs.** A new **Reparaciones** screen for the phones customers leave at the shop.
  - **Taking one in:** the customer's name and phone, the device, its IMEI or serial number, the fault, the estimate and, if one is paid, a deposit. Each repair gets a number of its own per year (`REP2026-000001`). Tiquet prints a resguardo for the customer and a slip that stays with the phone.
  - **Deposit:** it is a normal ticket ("Anticipo reparación REP2026-000001"). It counts in the cash drawer, the reports and VERI\*FACTU like any sale.
  - **Status:** Recibido, En curso, Listo, No reparado, Entregado. Every change is kept with who made it and when.
  - **Pick-up:** **Entregar y cobrar** asks for the final price, takes off the deposit and charges the rest in Venta. The ticket names the repair, and issuing it marks the phone as delivered.
  - **Never lost:** repairs are never deleted. One taken in by mistake is cancelled by an administrator, with a reason. Voiding the ticket that delivered a repair puts it back where it was.
  - **Invoices:** a ticket with a repair on it asks for the customer's details above 400 € (repairs are a service, RD 1619/2012 art. 4).
  - **Search:** by repair number, customer name, phone or IMEI, with filters by status.
- **Conditions on the resguardo:** a short text in **Ajustes > Ticket**, printed on every new resguardo.

How it works: [README](https://github.com/Ranjitbarnala0/tiquet#repairs). VERI\*FACTU is unchanged, and still off until it is switched on: [docs/verifactu.md](https://github.com/Ranjitbarnala0/tiquet/blob/main/docs/verifactu.md).
