"""Exact text replacements, the way Tiquet's hand-formatted code is changed
without a formatter touching anything else.

    python tools/patch.py <repo root> <edits.py>

edits.py defines EDITS = [(path, [(old, new), ...]), ...]. Every `old` must
occur exactly once in its file (after the earlier pairs of that file have been
applied); if any does not, nothing at all is written. Each file keeps its own
line endings (CRLF or LF).
"""

import os
import runpy
import sys


def main() -> None:
    if len(sys.argv) != 3:
        sys.exit(__doc__)
    root, edits = sys.argv[1], sys.argv[2]
    plan = runpy.run_path(edits)['EDITS']
    out = []
    for path, pairs in plan:
        full = os.path.join(root, path)
        with open(full, encoding='utf-8', newline='') as f:
            text = f.read()
        crlf = '\r\n' in text
        text = text.replace('\r\n', '\n')
        for old, new in pairs:
            n = text.count(old)
            if n != 1:
                sys.exit(f'{path}: found {n} times, needs exactly one; nothing written.\n---\n{old[:300]}')
            text = text.replace(old, new)
        out.append((full, path, text.replace('\n', '\r\n') if crlf else text))
    for full, path, text in out:
        with open(full, 'w', encoding='utf-8', newline='') as f:
            f.write(text)
        print('ok', path)


if __name__ == '__main__':
    main()
