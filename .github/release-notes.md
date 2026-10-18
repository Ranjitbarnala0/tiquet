Download `Tiquet_1.3.0_x64-setup.exe` below and run it. Runs on Windows 10 and 11 (64-bit) and needs no administrator rights. Built and tested on a clean GitHub Windows machine, not on a developer's PC.

The installer is not code-signed yet: if Windows says "Windows protected your PC", click **More info** and then **Run anyway**.

**Upgrading from 1.0, 1.1 or 1.2:** run this installer once over the old version. Nothing has to be set up again. From 1.3 on, Tiquet updates itself.

New in 1.3:

- **Updates itself.** A few seconds after it starts, and once a day, Tiquet asks for a newer version, never while a sale is on the screen. When there is one, a bar says "Hay una versión nueva" with **Actualizar ahora** and **Más tarde**. Updating downloads the new version, checks its signature, installs it silently and opens Tiquet again; sales and settings stay as they were. It can also be checked by hand in **Ajustes > Datos y copias > Buscar actualización**.
- **Network printers.** **Ajustes > Impresora** now chooses between a printer installed in Windows and a printer on the network, by its IP address and port (9100 on nearly all of them). Tickets go straight to the printer, and the test page, the character test and the cash drawer work over both. A printer that does not answer shows as disconnected, and a ticket that could not reach it is never marked as printed.
- **Start with Windows and full screen.** In **Ajustes > Datos y copias**: **Abrir Tiquet al encender el ordenador**, and **Pantalla completa**, which covers the whole screen, Windows taskbar included. Esc never leaves full screen; the button at the top right does. Both are off until switched on.
- **Guardar diagnóstico.** One button in **Ajustes > Datos y copias** saves a small zip in `Documents\Tiquet` with the activity log, the settings, the printers and the state of the database, to send when something goes wrong. It holds no database, no passwords, no certificate.
- **The till on the sale screen.** A small label above the ticket says **Caja abierta desde 9:00** or **Caja cerrada**, and opens the Caja screen. Charging cash with the till closed offers **Abrir caja** right in the charge window, with the opening float, and the sale carries on.
- **The first sale of the day no longer waits for the backup.** The daily backup of a till left open overnight now runs beside it, after the sale is answered.

VERI\*FACTU is unchanged, and still off until it is switched on: [docs/verifactu.md](https://github.com/Ranjitbarnala0/tiquet/blob/main/docs/verifactu.md).
