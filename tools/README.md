# Tools

Helpers for changing and releasing Tiquet. None of them is part of the app, and none starts a program: run them from your own shell. How they fit into the work is in [AGENTS.md](../AGENTS.md).

| Tool | What it does |
| --- | --- |
| `patch.py` | `py -3 -I tools/patch.py <repo root> <edits.py>`. Exact text replacements listed in `edits.py` as `EDITS = [(path, [(old, new), ...]), ...]`. Each `old` must occur exactly once, or nothing at all is written; files keep their line endings. The way hand-formatted code is changed without a formatter. |
| `soak.ps1` | `powershell -File tools/soak.ps1 -Installer <setup.exe> -Log <soak.log>`. Reads a fresh copy of the installer 14 times, 45 s apart, then lists what Windows Defender detected. Run before a release is published. |
| `prep-update-data.py` | `py -3 -I tools/prep-update-data.py <data folder> <copy>`. A throwaway copy of a data folder that signs its administrator in, prints nowhere and updates itself, for a real update test with an installed older version. |
| `cdp-eval.mjs` | `node tools/cdp-eval.mjs <port> "<expression>"`. Evaluates an expression in a page that has a debugging port and prints the result. |
| `cdp-shot.mjs` | `node tools/cdp-shot.mjs <port> <out.png>`. A screenshot of the page, also when its window is hidden. |
| `cdp-idle.mjs` | `node tools/cdp-idle.mjs <port> [seconds]`. How much script the page runs while untouched; an idle Tiquet page should be close to 0 ms. |

The `cdp-*` tools talk to the real app started hidden with a debugging port:

```
WEBVIEW2_USER_DATA_FOLDER=<empty folder> WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=9333 tiquet.exe --no-window --data <copy of a data folder>
```

`cdp.mjs` is the small connection they share.
