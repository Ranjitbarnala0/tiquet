# VERI\*FACTU in Tiquet

Tiquet 1.2 can work as a VERI\*FACTU billing system under Real Decreto 1007/2023 and Orden HAC/1177/2024. Every ticket, return and void gets a billing record (*registro de facturación*), chained to the one before by its SHA-256 *huella* and sent to the AEAT. Each ticket also carries the AEAT's QR code. The feature is **off** until it is switched on in **Ajustes > VERI\*FACTU**, so a shop that does not need it yet sees no change.

## Decisions

Taken on 2026-10-17.

| Question | Decision | Why |
| --- | --- | --- |
| Is it needed for the shop Tiquet was built for? | Build it for every case. The shop picks it up when the date comes. | The VAT regime is not confirmed yet, and the owner wants Tiquet to cover repairs, invoices with the customer's NIF and plain sales. Repairs are never under *recargo de equivalencia*, and any invoice the till issues falls under the regulation whatever the regime, so the work is needed either way. |
| VERI\*FACTU or "no VERI\*FACTU"? | **VERI\*FACTU only** (`TipoUsoPosibleSoloVerifactu` = S). | The owner asked for the simplest thing that works on the shop's PC and for no more questions. VERI\*FACTU needs no electronic signature of the records, no event log and no local archive to guard: the AEAT keeps the records. |
| Producer details (name, NIF, address) in every record and in the *declaración responsable* | **Left empty for now.** Ajustes refuses to switch VERI\*FACTU on while the name or NIF is missing (`E_PRODUCER`). | The owner chose a placeholder. Fill in the constants at the top of `src-tauri/src/verifactu.rs` (or set `TIQUET_PRODUCER_NAME`, `TIQUET_PRODUCER_NIF` and `TIQUET_PRODUCER_ADDRESS` when building), plus the date and place the declaration is signed. |

## Dates and rules, checked on 2026-10-17

- **In force:** RD 1007/2023, final provision 4, as worded by RDL 15/2025 (BOE 3 Dec 2025, validated by the Congreso on 11 Dec 2025). Corporate-tax payers must comply **before 1 Jan 2027**. Everyone else, including autónomos, must comply **before 1 Jul 2027**.
- **Announced, not yet approved:** on 5 Oct 2026 Hacienda said it plans to postpone the pending obligations **to October 2028**, to line up with mandatory e-invoicing. Some technical aspects may change before then.
- **Producers:** have had to offer only adapted systems since 29 Jul 2025. Every version needs its *declaración responsable*, shown inside the program.
- **Sources:**
  - [AEAT VERI\*FACTU page](https://sede.agenciatributaria.gob.es/Sede/iva/sistemas-informaticos-facturacion-verifactu.html)
  - [BOE RDL 15/2025](https://www.boe.es/boe/dias/2025/12/03/pdfs/BOE-A-2025-24446.pdf)
  - [Hacienda note of 5 Oct 2026](https://www.hacienda.gob.es/sgt/gabsehacienda/nota-informativa-verifactu.pdf)
  - The AEAT developer documents below.

| AEAT document | Version used |
| --- | --- |
| Huella (hash) specification | 0.1.2, 27/08/2024 |
| QR specification | 0.5.0, 10/12/2025 |
| Web service description | 1.0.3, 28/07/2025 |
| Validaciones y errores | 1.2.2, 08/04/2026 |
| XSD schemas (`SuministroLR.xsd`, `SuministroInformacion.xsd`, `RespuestaSuministro.xsd`) | as published, January 2026 |

## What gets a record

| In Tiquet | AEAT record | Notes |
| --- | --- | --- |
| Ticket (`T…`) | Alta, `F2` simplified invoice | One `DetalleDesglose` per VAT rate, `ClaveRegimen` 01, `S1`. No recipient. |
| Full invoice (`TF…`) | Alta, `F1` | Recipient by NIF, or by EU VAT number as `IDOtro` type 02. |
| Return of a ticket (`TR…`) | Alta, `R5`, `TipoRectificativa` I | Negative amounts. Points at the ticket it corrects. Carries `FechaOperacion` when the sale was on another day. |
| Return of an invoice (`TFR…`) | Alta, `R1`, `TipoRectificativa` I | A return is article 80.Dos LIVA. Same recipient as the invoice. |
| Void (same day, ticket issued by mistake) | Anulación | `SinRegistroPrevio` S if the ticket was issued before VERI\*FACTU was on. |
| Practice ticket (training mode) | none | Not an invoice. |

## How it works

- **Same transaction.** The record is written inside the IMMEDIATE transaction that issues the ticket. Both are committed together or not at all, and nothing can be inserted between a ticket and its record.
- **Storage.**
  - **Records:** `fiscal_records` (schema v7). Triggers forbid UPDATE and DELETE.
  - **AEAT answers:** `fiscal_sends` holds the AEAT's answer to each record: state, error code, message, CSV.
  - **Corrections:** a correction is a new record (`corrects` points at the one it fixes). Nothing is ever rewritten.
- **One chain per AEAT environment.** Test (preproducción) and real records chain separately, because the AEAT keeps them apart. The first real record therefore says `PrimerRegistro` S even after testing.
- **Huella.** SHA-256 of `campo=valor&…`, exactly as the AEAT specifies. Amounts are always written with two decimals, in both the XML and the hash, because `41.4` and `41.40` hash differently.
- **QR.**
  - **What it encodes:** `ValidarQR` with `nif`, `numserie`, `fecha` and `importe`, URL-encoded like the specification's own Java example.
  - **How it looks on the ticket:** error correction M, 30–40 mm (34 mm on an 8 dots/mm head), with four modules of white around it (3 mm for Tiquet's addresses). It sits at the top, right under the logo: "QR tributario:" above it and "VERI\*FACTU" below.
  - **How it is printed:** it reaches the printer as a raster image built from the same modules the screen preview draws, so it does not depend on each clone's QR command.
  - **Reprints:** a reprint uses the QR stored with the ticket's record.
- **Sending** (`src-tauri/src/aeat.rs`).
  - **Who sends:** a background thread, with its own database connection each round.
  - **Certificate:** chosen in Ajustes from the Windows user's personal store. Only certificates with client authentication and a usable private key are listed. The certificate itself never leaves Windows; Tiquet keeps only its thumbprint.
  - **Transport:** TLS through Windows SChannel.
  - **Message:** up to 1,000 records per message, oldest first, one issuer per message.
  - **Pace:** the next message waits as long as the AEAT's last `TiempoEsperaEnvio` says (60 s to begin with). A new record wakes the sender at once.
  - **Late records:** records held back by a failure, or older than 5 minutes, go in a message marked `Incidencia` S.
  - **Retries:** failures retry after 1, 2, 4, then every 8 minutes. The legal minimum is once an hour.
  - **Selling never waits for any of this.**
- **Telling people.**
  - **The bar:** a bar under the header tells whoever is at the till when records cannot be sent, or when the AEAT asks for a correction (OM art. 16).
  - **Ajustes:** shows what is waiting, the last send, the last error and the PC clock's drift against the AEAT's clock. Each record the AEAT refused, or accepted with errors, has a **Corregir y reenviar** button. It sends a *subsanación* (`Subsanacion` S, plus `RechazoPrevio` X or S as the case needs), built from the stored ticket and the shop's current details.
  - **Each ticket:** shows where it stands with the AEAT.
- **Restoring a backup** never loses records. Those made after the backup was taken are copied back from the replaced database, with their AEAT answers, and the chain carries on from the newest one. A damaged database gives up whatever can still be read.
- **Guards.**
  - **Before switching on:** VERI\*FACTU cannot be switched on without the producer details, a valid shop NIF and an owner name.
  - **NIF lock:** once real records exist, the shop NIF cannot change, because Tiquet declares itself single-taxpayer (`TipoUsoPosibleMultiOT` N).
  - **VAT rates:** while VERI\*FACTU is on, only the rates the AEAT accepts today (21, 10, 4 and 0 %) can be sold. Any other rate would be refused, and the invoice would have to be issued again.
- **Declaración responsable:** Ajustes > VERI\*FACTU > **Declaración responsable** shows it, generated for the running version.

## How it was checked

- **Hash:** the AEAT's worked examples (first record, chained record, anulación, and the signed R3 example with one-decimal amounts) are unit tests.
- **The full chain** is a unit test: two-rate ticket, full invoice, R5 return, void and practice ticket. Every huella is recomputed from the stored fields, every link is checked, and records are shown to be immutable. Further unit tests cover corrections, restoring a backup, the NIF lock, the VAT-rate guard, AEAT reply parsing (including duplicates and SOAP faults), HTTP chunking and certificate parsing.
- **Independent check:** a Python script recomputed every huella from the XML stored by a running Tiquet, using the AEAT specification and not Tiquet's code. All matched, and the message validated against the official `SuministroLR.xsd`. A deliberately broken copy was rejected.
- **The printed QR,** taken from the ESC/POS bytes, decoded with ZXing: a QR code with error correction M, the exact AEAT URL, 33.8 × 33.8 mm.
- **TLS:** Tiquet's own client fetched the AEAT's WSDL from both the test and the real hosts (HTTP 200).

## Not done, or still to confirm

- **No record has reached the AEAT yet.** That needs the shop owner's certificate. The first step at the shop:
  1. Choose **Pruebas AEAT**.
  2. Pick the certificate.
  3. Sell a ticket and see it turn "registrado en la AEAT".
  4. Switch to **Real**.
- **Producer details:** the producer details and the date and place of the *declaración responsable* are placeholders (see Decisions).
- **`ClaveRegimen`:** always `01`. Whether a retailer in *recargo de equivalencia* should use `18` on its own sales is not stated anywhere official. Ask the shop's gestoría; it is a one-line change.
- **Repair invoice limit:** repairs above 400 € cannot go on a simplified invoice (RD 1619/2012 art. 4: the 3,000 € limit is for retail sales). Tiquet does not tell products from services yet.
- **`F3`:** a full invoice that replaces earlier tickets (`F3`) is not supported. The customer's details have to be given at the sale.
- **Negative QR amounts:** the QR specification does not say whether a negative `importe` (returns) is accepted. Tiquet prints the record's own amount. Check one in Pruebas.
- **Other mode:** the "no VERI\*FACTU" mode, its XAdES signatures and its event log are not implemented. They are not required for a VERI\*FACTU-only system.
- **Once it is mandatory:** after the date applies, a shop that has started sending must stay on VERI\*FACTU until 31 December. Ajustes says so but does not enforce it.
