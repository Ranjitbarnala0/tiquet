Download `Tiquet_1.2.0_x64-setup.exe` below and run it. Runs on Windows 10 and 11 (64-bit) and needs no administrator rights. Built and tested on a clean GitHub Windows machine, not on a developer's PC.

The installer is not code-signed yet: if Windows says "Windows protected your PC", click **More info** and then **Run anyway**.

**Upgrading from 1.0 or 1.1:** just run it. The database upgrades itself on first start, after a copy of it is kept, and nothing has to be set up again.

New in 1.2:

- **VERI\*FACTU** (Real Decreto 1007/2023), **off until it is switched on** in Ajustes > VERI\*FACTU. Nothing changes for a shop that leaves it off. With it on:
  - **Records:** every ticket, return and void leaves a billing record in the AEAT's format, chained to the one before by its SHA-256 huella. The record is written in the same transaction as the ticket and can never be changed or deleted.
  - **Sending:** Tiquet sends the records to the AEAT by itself, with the shop owner's electronic certificate installed in Windows, and waits between messages as long as the AEAT asks. Without internet the till keeps selling, and the records go later, marked as delayed by an incident.
  - **QR:** the ticket prints the AEAT's QR (30–40 mm), labelled "QR tributario:" above it and "VERI\*FACTU" below.
  - **Warnings:** a bar under the header warns when records are waiting or the AEAT asks for a correction. The correction is one button away in Ajustes, and every ticket shows where it stands with the AEAT.
  - **Test or real:** the AEAT's test environment and the real one are separate choices, each with its own chain.
  - **Backups:** restoring a backup never loses billing records.
  - **VAT rates:** only the rates the AEAT accepts (21, 10, 4 and 0 %) can be sold while it is on.
- **This build cannot switch VERI\*FACTU on yet.** It does not carry the producer's details, which every record must include. For autónomos the obligation starts on 1 July 2027, and Hacienda has announced it plans to move it to October 2028.

How it works and what is still open: [docs/verifactu.md](https://github.com/Ranjitbarnala0/tiquet/blob/main/docs/verifactu.md).
