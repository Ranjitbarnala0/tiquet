"""A throwaway copy of a Tiquet data folder for a real update test.

    py -3 -I tools/prep-update-data.py <data folder> <copy>

The copy signs its first administrator in when Tiquet opens, prints nowhere
(no printer set), updates itself and takes no backups, so an installed older
version can be run hidden on it:

    TIQUET_UPDATE_URL=<the new release's latest.json> tiquet.exe --no-window --data <copy>

Then read <copy>/tiquet.log: "downloading", "installing" about 2 minutes
later, and the new version "starting". The original folder is never touched.
"""

import json
import os
import shutil
import sqlite3
import sys


def main() -> None:
    if len(sys.argv) != 3:
        sys.exit(__doc__)
    src, dst = sys.argv[1], sys.argv[2]
    if os.path.abspath(src) == os.path.abspath(dst):
        sys.exit('the copy must be another folder')
    shutil.rmtree(dst, ignore_errors=True)
    shutil.copytree(src, dst)
    db = sqlite3.connect(os.path.join(dst, 'tiquet.db'))
    s = json.loads(db.execute("SELECT value FROM settings WHERE key = 'settings'").fetchone()[0])
    admin = db.execute("SELECT id FROM users WHERE role = 'admin' AND active = 1 ORDER BY id LIMIT 1").fetchone()
    if admin:
        s['autoLogin'] = admin[0]
    s['printerName'] = ''
    s['printerKind'] = 'windows'
    s['invoicePrinter'] = ''
    s['autoUpdate'] = True
    s['autoBackup'] = False
    db.execute("UPDATE settings SET value = ? WHERE key = 'settings'", (json.dumps(s),))
    db.execute("DELETE FROM settings WHERE key = 'resumeUser'")
    db.commit()
    print('schema', db.execute('PRAGMA user_version').fetchone()[0], 'tickets', db.execute('SELECT COUNT(*) FROM tickets').fetchone()[0], 'auto sign-in', admin[0] if admin else None)
    db.execute('PRAGMA wal_checkpoint(TRUNCATE)')
    db.close()


if __name__ == '__main__':
    main()
