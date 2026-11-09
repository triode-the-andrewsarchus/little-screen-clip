#!/usr/bin/env python3
"""Create an owned, deterministic SQLite database with paging and declared relationships."""
import sqlite3
from pathlib import Path


def make(path):
    path.unlink(missing_ok=True)
    with sqlite3.connect(path) as database:
        database.executescript('''PRAGMA page_size=4096;
PRAGMA user_version=7;
PRAGMA application_id=1279478864;
CREATE TABLE ports(id INTEGER PRIMARY KEY,name TEXT NOT NULL);
CREATE TABLE sailings(id INTEGER PRIMARY KEY,port_id INTEGER REFERENCES ports(id),notes TEXT,manifest BLOB);
CREATE VIEW summary AS SELECT name FROM ports;
INSERT INTO ports VALUES(1,'North harbor'),(2,'Café pier');
''')
        database.executemany('INSERT INTO sailings VALUES(?,?,?,?)',[(index,1 if index%2 else 2,'Morning crossing' if index==1 else 'Evening arrival' if index==205 else f'Crossing {index:03d}',b'route details') for index in range(1,206)])


if __name__=='__main__':
    make(Path(__file__).resolve().parents[1]/'crates/clip-workbench/tests/fixtures/database/harbor.sqlite')
