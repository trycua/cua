import { Database } from 'bun:sqlite';
import { getDbPath } from './config';

function getDb(): Database {
  const db = new Database(getDbPath());
  db.exec('PRAGMA journal_mode = WAL;');
  db.exec(
    'CREATE TABLE IF NOT EXISTS kv (k TEXT PRIMARY KEY, v TEXT NOT NULL);'
  );
  return db;
}

export function setApiKey(token: string) {
  const db = getDb();
  try {
    const stmt = db.query(
      "INSERT INTO kv (k, v) VALUES ('api_key', ?) ON CONFLICT(k) DO UPDATE SET v=excluded.v"
    );
    stmt.run(token);
  } finally {
    db.close();
  }
}

export function getApiKey(): string | null {
  const db = getDb();
  try {
    const row = db.query("SELECT v FROM kv WHERE k='api_key'").get() as
      | { v: string }
      | undefined;
    return row?.v ?? null;
  } finally {
    db.close();
  }
}

export function clearApiKey() {
  const db = getDb();
  try {
    db.query("DELETE FROM kv WHERE k='api_key'").run();
  } finally {
    db.close();
  }
}
